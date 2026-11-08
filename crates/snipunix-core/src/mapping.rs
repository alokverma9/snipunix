//! Mapping between virtual-desktop logical coordinates and captured image
//! pixels.

use thiserror::Error;

use crate::geometry::LogicalRect;
use crate::layout::Monitor;

/// Size of the image returned by the screenshot portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSize {
    pub width: u32,
    pub height: u32,
}

impl ImageSize {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// A rectangle in captured-image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ImageRect {
    #[must_use]
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Bidirectional scale between the logical desktop bounding box and the
/// captured image.
///
/// The portal does not report the geometry of the captured desktop. M0 derives
/// one uniform scale per axis from the image size and the GDK monitor layout.
/// Mixed-DPI desktops where monitors have different effective scales are a
/// documented limitation of this milestone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureMapping {
    desktop: LogicalRect,
    image: ImageSize,
    scale_x: f64,
    scale_y: f64,
}

impl CaptureMapping {
    pub fn new(desktop: LogicalRect, image: ImageSize) -> Result<Self, MappingError> {
        if desktop.is_empty() {
            return Err(MappingError::EmptyDesktop);
        }
        if image.width == 0 || image.height == 0 {
            return Err(MappingError::EmptyImage);
        }
        Ok(Self {
            desktop,
            image,
            scale_x: f64::from(image.width) / desktop.width,
            scale_y: f64::from(image.height) / desktop.height,
        })
    }

    #[must_use]
    pub fn desktop(&self) -> LogicalRect {
        self.desktop
    }

    #[must_use]
    pub fn image(&self) -> ImageSize {
        self.image
    }

    #[must_use]
    pub fn scale_x(&self) -> f64 {
        self.scale_x
    }

    #[must_use]
    pub fn scale_y(&self) -> f64 {
        self.scale_y
    }

    /// Converts a logical selection to an image crop.
    ///
    /// The rectangle is clamped to the captured desktop and the edges are
    /// rounded outward so a selection never loses a partially covered pixel.
    #[must_use]
    pub fn virtual_to_image(&self, selection: LogicalRect) -> Option<ImageRect> {
        let selection = selection.intersection(self.desktop)?;
        let left = ((selection.x - self.desktop.x) * self.scale_x).floor();
        let top = ((selection.y - self.desktop.y) * self.scale_y).floor();
        let right = ((selection.right() - self.desktop.x) * self.scale_x).ceil();
        let bottom = ((selection.bottom() - self.desktop.y) * self.scale_y).ceil();

        let left = left.clamp(0.0, f64::from(self.image.width)) as u32;
        let top = top.clamp(0.0, f64::from(self.image.height)) as u32;
        let right = right.clamp(0.0, f64::from(self.image.width)) as u32;
        let bottom = bottom.clamp(0.0, f64::from(self.image.height)) as u32;

        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);
        if width == 0 || height == 0 {
            return None;
        }
        Some(ImageRect::new(left, top, width, height))
    }

    /// Image pixels occupied by one monitor.
    #[must_use]
    pub fn image_rect_for_monitor(&self, monitor: &Monitor) -> Option<ImageRect> {
        self.virtual_to_image(monitor.rect())
    }

    /// Scale needed to draw image pixels as logical pixels on screen.
    #[must_use]
    pub fn logical_scale(&self) -> (f64, f64) {
        (1.0 / self.scale_x, 1.0 / self.scale_y)
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum MappingError {
    #[error("desktop bounding box is empty")]
    EmptyDesktop,
    #[error("captured image has a zero dimension")]
    EmptyImage,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{LogicalPoint, LogicalSize};

    #[test]
    fn maps_identity_at_scale_one() {
        let mapping = CaptureMapping::new(
            LogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
            ImageSize::new(1920, 1080),
        )
        .unwrap();
        assert_eq!(
            mapping.virtual_to_image(LogicalRect::new(10.0, 20.0, 100.0, 50.0)),
            Some(ImageRect::new(10, 20, 100, 50))
        );
    }

    #[test]
    fn maps_scaled_desktop_and_rounds_outward() {
        let mapping = CaptureMapping::new(
            LogicalRect::new(0.0, 0.0, 960.0, 540.0),
            ImageSize::new(1920, 1080),
        )
        .unwrap();
        assert_eq!(mapping.scale_x(), 2.0);
        assert_eq!(
            mapping.virtual_to_image(LogicalRect::new(10.25, 20.5, 100.0, 50.0)),
            Some(ImageRect::new(20, 41, 201, 100))
        );
    }

    #[test]
    fn clamps_to_image_bounds() {
        let mapping = CaptureMapping::new(
            LogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
            ImageSize::new(1920, 1080),
        )
        .unwrap();
        assert_eq!(
            mapping.virtual_to_image(LogicalRect::new(1900.0, 1000.0, 500.0, 500.0)),
            Some(ImageRect::new(1900, 1000, 20, 80))
        );
    }

    #[test]
    fn handles_negative_monitor_origins() {
        let desktop = LogicalRect::new(-1280.0, 0.0, 3200.0, 1080.0);
        let mapping =
            CaptureMapping::new(desktop, ImageSize::new(3200, 1080)).expect("valid mapping");
        let monitor = Monitor {
            id: 1,
            name: "left".into(),
            origin: LogicalPoint::new(-1280.0, 0.0),
            size: LogicalSize::new(1280.0, 1024.0),
            scale: 1.0,
        };
        assert_eq!(
            mapping.image_rect_for_monitor(&monitor),
            Some(ImageRect::new(0, 0, 1280, 1024))
        );
    }

    #[test]
    fn rejects_degenerate_inputs() {
        assert!(matches!(
            CaptureMapping::new(
                LogicalRect::new(0.0, 0.0, 0.0, 10.0),
                ImageSize::new(10, 10)
            ),
            Err(MappingError::EmptyDesktop)
        ));
        assert!(matches!(
            CaptureMapping::new(
                LogicalRect::new(0.0, 0.0, 10.0, 10.0),
                ImageSize::new(0, 10)
            ),
            Err(MappingError::EmptyImage)
        ));
    }
}
