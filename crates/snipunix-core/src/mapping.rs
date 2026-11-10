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

/// Pixel rectangle occupied by one monitor in the captured image.
///
/// Backends that can report exact monitor geometry (X11 through RandR) fill
/// these in. Backends that only return a composited desktop image (the
/// Screenshot portal) leave the list empty and the mapping falls back to one
/// uniform scale.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorImage {
    /// Stable monitor identifier; matches [`crate::Monitor::id`] when known.
    pub id: u32,
    /// Monitor rectangle in virtual-desktop logical coordinates.
    pub logical: LogicalRect,
    /// Monitor rectangle in captured-image pixels.
    pub image: ImageRect,
}

/// Bidirectional scale between the logical desktop bounding box and the
/// captured image.
///
/// A mapping is either *uniform* (one scale per axis, used when the backend
/// cannot report per-monitor geometry) or *per-monitor* (each monitor has its
/// own image rectangle, which keeps mixed-DPI crops inside a monitor
/// pixel-exact).
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureMapping {
    desktop: LogicalRect,
    image: ImageSize,
    scale_x: f64,
    scale_y: f64,
    monitors: Vec<MonitorImage>,
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
            monitors: Vec::new(),
        })
    }

    /// Builds a per-monitor mapping from exact monitor rectangles.
    ///
    /// This is what makes mixed-DPI crops inside a monitor exact: each monitor
    /// keeps its own scale derived from its logical and pixel rectangles. A
    /// selection spanning monitors with different scales uses the bounding box
    /// of the per-monitor pieces. Every monitor rectangle must be non-empty and
    /// inside the captured image.
    pub fn with_monitors(
        desktop: LogicalRect,
        image: ImageSize,
        monitors: Vec<MonitorImage>,
    ) -> Result<Self, MappingError> {
        let mut mapping = Self::new(desktop, image)?;
        for monitor in &monitors {
            if monitor.logical.is_empty() {
                return Err(MappingError::EmptyMonitor { id: monitor.id });
            }
            if monitor.image.width == 0 || monitor.image.height == 0 {
                return Err(MappingError::EmptyMonitor { id: monitor.id });
            }
            if monitor.image.x.saturating_add(monitor.image.width) > image.width
                || monitor.image.y.saturating_add(monitor.image.height) > image.height
            {
                return Err(MappingError::MonitorOutsideImage {
                    id: monitor.id,
                    x: monitor.image.x,
                    y: monitor.image.y,
                    width: monitor.image.width,
                    height: monitor.image.height,
                });
            }
        }
        mapping.monitors = monitors;
        Ok(mapping)
    }

    /// `true` when the mapping has exact per-monitor rectangles.
    #[must_use]
    pub fn is_per_monitor(&self) -> bool {
        !self.monitors.is_empty()
    }

    /// Exact per-monitor rectangles, or an empty slice for a uniform mapping.
    #[must_use]
    pub fn monitor_images(&self) -> &[MonitorImage] {
        &self.monitors
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
        if self.monitors.is_empty() {
            return self.uniform_to_image(selection);
        }

        // Per-monitor mapping: split the selection at monitor edges and map
        // each piece with that monitor's own scale. The result is the bounding
        // box of the mapped pieces, which is exact for a selection inside one
        // monitor and conservative for a selection spanning monitors.
        let mut left = f64::INFINITY;
        let mut top = f64::INFINITY;
        let mut right = f64::NEG_INFINITY;
        let mut bottom = f64::NEG_INFINITY;
        for monitor in &self.monitors {
            let Some(part) = selection.intersection(monitor.logical) else {
                continue;
            };
            let scale_x = f64::from(monitor.image.width) / monitor.logical.width;
            let scale_y = f64::from(monitor.image.height) / monitor.logical.height;
            let piece_left = f64::from(monitor.image.x) + (part.x - monitor.logical.x) * scale_x;
            let piece_top = f64::from(monitor.image.y) + (part.y - monitor.logical.y) * scale_y;
            let piece_right =
                f64::from(monitor.image.x) + (part.right() - monitor.logical.x) * scale_x;
            let piece_bottom =
                f64::from(monitor.image.y) + (part.bottom() - monitor.logical.y) * scale_y;
            left = left.min(piece_left.floor());
            top = top.min(piece_top.floor());
            right = right.max(piece_right.ceil());
            bottom = bottom.max(piece_bottom.ceil());
        }
        self.clamp_image_rect(left, top, right, bottom)
    }

    fn uniform_to_image(&self, selection: LogicalRect) -> Option<ImageRect> {
        let left = ((selection.x - self.desktop.x) * self.scale_x).floor();
        let top = ((selection.y - self.desktop.y) * self.scale_y).floor();
        let right = ((selection.right() - self.desktop.x) * self.scale_x).ceil();
        let bottom = ((selection.bottom() - self.desktop.y) * self.scale_y).ceil();
        self.clamp_image_rect(left, top, right, bottom)
    }

    fn clamp_image_rect(&self, left: f64, top: f64, right: f64, bottom: f64) -> Option<ImageRect> {
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
        self.monitors
            .iter()
            .find(|mapping| mapping.id == monitor.id)
            .map(|mapping| mapping.image)
            .or_else(|| self.virtual_to_image(monitor.rect()))
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
    #[error("monitor {id} has an empty logical or pixel rectangle")]
    EmptyMonitor { id: u32 },
    #[error("monitor {id} image rectangle {x},{y} {width}x{height} is outside the captured image")]
    MonitorOutsideImage {
        id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
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
    fn per_monitor_mapping_is_exact_inside_each_monitor() {
        // Left monitor: 960x540 at 2x -> 1920x1080 pixels.
        // Right monitor: 1280x720 at 1x -> 1280x720 pixels, placed to the right.
        let mapping = CaptureMapping::with_monitors(
            LogicalRect::new(0.0, 0.0, 2240.0, 1080.0),
            ImageSize::new(3200, 1080),
            vec![
                MonitorImage {
                    id: 0,
                    logical: LogicalRect::new(0.0, 0.0, 960.0, 540.0),
                    image: ImageRect::new(0, 0, 1920, 1080),
                },
                MonitorImage {
                    id: 1,
                    logical: LogicalRect::new(960.0, 0.0, 1280.0, 720.0),
                    image: ImageRect::new(1920, 0, 1280, 720),
                },
            ],
        )
        .unwrap();

        assert!(mapping.is_per_monitor());
        // Logical (10,10)-(20,20) on the 2x monitor becomes (20,20)-(40,40).
        assert_eq!(
            mapping.virtual_to_image(LogicalRect::new(10.0, 10.0, 10.0, 10.0)),
            Some(ImageRect::new(20, 20, 20, 20))
        );
        // Logical (970,10)-(980,20) on the 1x monitor becomes (1930,10)-(1940,20).
        assert_eq!(
            mapping.virtual_to_image(LogicalRect::new(970.0, 10.0, 10.0, 10.0)),
            Some(ImageRect::new(1930, 10, 10, 10))
        );
    }

    #[test]
    fn per_monitor_mapping_clamps_and_rejects_outside_monitors() {
        let mapping = CaptureMapping::with_monitors(
            LogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
            ImageSize::new(1920, 1080),
            vec![MonitorImage {
                id: 0,
                logical: LogicalRect::new(0.0, 0.0, 1920.0, 1080.0),
                image: ImageRect::new(0, 0, 1920, 1080),
            }],
        )
        .unwrap();
        // A triangle outside the only monitor maps to nothing.
        assert_eq!(
            mapping.virtual_to_image(LogicalRect::new(2000.0, 2000.0, 10.0, 10.0)),
            None
        );
        // Selections crossing the edge are clamped to the image.
        assert_eq!(
            mapping.virtual_to_image(LogicalRect::new(1900.0, 1000.0, 500.0, 500.0)),
            Some(ImageRect::new(1900, 1000, 20, 80))
        );
    }

    #[test]
    fn rejects_invalid_monitor_rectangles() {
        assert!(matches!(
            CaptureMapping::with_monitors(
                LogicalRect::new(0.0, 0.0, 100.0, 100.0),
                ImageSize::new(100, 100),
                vec![MonitorImage {
                    id: 3,
                    logical: LogicalRect::new(0.0, 0.0, 100.0, 100.0),
                    image: ImageRect::new(0, 0, 0, 100),
                }],
            ),
            Err(MappingError::EmptyMonitor { id: 3 })
        ));
        assert!(matches!(
            CaptureMapping::with_monitors(
                LogicalRect::new(0.0, 0.0, 100.0, 100.0),
                ImageSize::new(100, 100),
                vec![MonitorImage {
                    id: 4,
                    logical: LogicalRect::new(0.0, 0.0, 100.0, 100.0),
                    image: ImageRect::new(50, 0, 100, 100),
                }],
            ),
            Err(MappingError::MonitorOutsideImage { id: 4, .. })
        ));
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
