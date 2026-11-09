//! Screen-space view transform for the editor canvas.
//!
//! The transform is the only place where zoom and pan exist. Document geometry
//! remains in original-image pixels, so changing the viewport never mutates an
//! annotation or the crop.

use crate::geometry::{DocPoint, DocRect};

/// A point in GTK logical (screen/widget) coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

impl ScreenPoint {
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A rectangle in GTK logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenRect {
    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn right(self) -> f64 {
        self.x + self.width
    }

    #[must_use]
    pub fn bottom(self) -> f64 {
        self.y + self.height
    }

    #[must_use]
    pub fn contains(self, point: ScreenPoint) -> bool {
        point.x >= self.x && point.y >= self.y && point.x < self.right() && point.y < self.bottom()
    }
}

/// A size in GTK logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenSize {
    pub width: f64,
    pub height: f64,
}

impl ScreenSize {
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub fn is_usable(self) -> bool {
        self.width > 1.0 && self.height > 1.0
    }
}

/// Smallest and largest editor zoom factors.
pub const MIN_ZOOM: f64 = 0.05;
pub const MAX_ZOOM: f64 = 16.0;
/// Empty border around the image when fitting it to the window.
pub const FIT_MARGIN: f64 = 28.0;

/// Maps document pixels to widget coordinates and back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    zoom: f64,
    pan: ScreenPoint,
    crop: DocRect,
    viewport: ScreenSize,
}

impl ViewTransform {
    #[must_use]
    pub fn new(zoom: f64, pan: ScreenPoint, crop: DocRect, viewport: ScreenSize) -> Self {
        let mut transform = Self {
            zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM),
            pan,
            crop,
            viewport,
        };
        transform.pan = transform.constrained_pan();
        transform
    }

    #[must_use]
    pub fn fit(crop: DocRect, viewport: ScreenSize) -> Self {
        if !viewport.is_usable() || crop.is_empty() {
            return Self::new(1.0, ScreenPoint::new(0.0, 0.0), crop, viewport);
        }
        let available_width = (viewport.width - FIT_MARGIN * 2.0).max(1.0);
        let available_height = (viewport.height - FIT_MARGIN * 2.0).max(1.0);
        let zoom = (available_width / crop.width)
            .min(available_height / crop.height)
            .clamp(MIN_ZOOM, MAX_ZOOM);
        let image_width = crop.width * zoom;
        let image_height = crop.height * zoom;
        Self::new(
            zoom,
            ScreenPoint::new(
                (viewport.width - image_width) / 2.0,
                (viewport.height - image_height) / 2.0,
            ),
            crop,
            viewport,
        )
    }

    #[must_use]
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    #[must_use]
    pub fn pan(&self) -> ScreenPoint {
        self.pan
    }

    #[must_use]
    pub fn crop(&self) -> DocRect {
        self.crop
    }

    #[must_use]
    pub fn viewport(&self) -> ScreenSize {
        self.viewport
    }

    #[must_use]
    pub fn image_rect_on_screen(&self) -> ScreenRect {
        ScreenRect::new(
            self.pan.x,
            self.pan.y,
            self.crop.width * self.zoom,
            self.crop.height * self.zoom,
        )
    }

    #[must_use]
    pub fn screen_from_doc(&self, point: DocPoint) -> ScreenPoint {
        ScreenPoint::new(
            (point.x - self.crop.x) * self.zoom + self.pan.x,
            (point.y - self.crop.y) * self.zoom + self.pan.y,
        )
    }

    #[must_use]
    pub fn doc_from_screen(&self, point: ScreenPoint) -> DocPoint {
        DocPoint::new(
            (point.x - self.pan.x) / self.zoom + self.crop.x,
            (point.y - self.pan.y) / self.zoom + self.crop.y,
        )
    }

    #[must_use]
    pub fn screen_rect_from_doc(&self, rect: DocRect) -> ScreenRect {
        let origin = self.screen_from_doc(rect.origin());
        ScreenRect::new(
            origin.x,
            origin.y,
            rect.width * self.zoom,
            rect.height * self.zoom,
        )
    }

    /// Intersection of the crop and the visible viewport, in document pixels.
    #[must_use]
    pub fn visible_doc_rect(&self) -> Option<DocRect> {
        let top_left = self.doc_from_screen(ScreenPoint::new(0.0, 0.0));
        let bottom_right =
            self.doc_from_screen(ScreenPoint::new(self.viewport.width, self.viewport.height));
        DocRect::from_corners(top_left, bottom_right).intersection(self.crop)
    }

    /// Zooms around a screen-space anchor so the document point under the
    /// pointer stays under the pointer.
    #[must_use]
    pub fn zoomed_at(&self, anchor: ScreenPoint, zoom: f64) -> Self {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let document = self.doc_from_screen(anchor);
        let pan = ScreenPoint::new(
            anchor.x - (document.x - self.crop.x) * zoom,
            anchor.y - (document.y - self.crop.y) * zoom,
        );
        Self::new(zoom, pan, self.crop, self.viewport)
    }

    #[must_use]
    pub fn panned_by(&self, dx: f64, dy: f64) -> Self {
        Self::new(
            self.zoom,
            ScreenPoint::new(self.pan.x + dx, self.pan.y + dy),
            self.crop,
            self.viewport,
        )
    }

    #[must_use]
    pub fn with_viewport(&self, viewport: ScreenSize) -> Self {
        Self::new(self.zoom, self.pan, self.crop, viewport)
    }

    #[must_use]
    pub fn with_crop(&self, crop: DocRect) -> Self {
        Self::new(self.zoom, self.pan, crop, self.viewport)
    }

    fn constrained_pan(&self) -> ScreenPoint {
        let image_width = self.crop.width * self.zoom;
        let image_height = self.crop.height * self.zoom;
        let x = if image_width <= self.viewport.width {
            (self.viewport.width - image_width) / 2.0
        } else {
            self.pan.x.clamp(self.viewport.width - image_width, 0.0)
        };
        let y = if image_height <= self.viewport.height {
            (self.viewport.height - image_height) / 2.0
        } else {
            self.pan.y.clamp(self.viewport.height - image_height, 0.0)
        };
        ScreenPoint::new(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crop() -> DocRect {
        DocRect::new(100.0, 50.0, 800.0, 600.0)
    }

    fn viewport() -> ScreenSize {
        ScreenSize::new(1000.0, 800.0)
    }

    #[test]
    fn screen_and_document_mapping_round_trips() {
        let view = ViewTransform::new(1.5, ScreenPoint::new(20.0, 30.0), crop(), viewport());
        let document = DocPoint::new(240.0, 180.0);
        let screen = view.screen_from_doc(document);
        assert_eq!(view.doc_from_screen(screen), document);
    }

    #[test]
    fn fit_centres_non_square_crop() {
        let view = ViewTransform::fit(crop(), viewport());
        assert!((view.zoom() - (944.0 / 800.0)).abs() < 1e-9);
        let rect = view.image_rect_on_screen();
        assert!((rect.x - (1000.0 - rect.width) / 2.0).abs() < 1e-9);
        assert!((rect.y - (800.0 - rect.height) / 2.0).abs() < 1e-9);
    }

    #[test]
    fn zoom_keeps_the_anchor_stable() {
        let view = ViewTransform::fit(crop(), viewport());
        let anchor = ScreenPoint::new(300.0, 250.0);
        let before = view.doc_from_screen(anchor);
        let zoomed = view.zoomed_at(anchor, view.zoom() * 2.0);
        let after = zoomed.doc_from_screen(anchor);
        assert!((before.x - after.x).abs() < 1e-6);
        assert!((before.y - after.y).abs() < 1e-6);
    }

    #[test]
    fn pan_is_constrained_to_the_image() {
        let view = ViewTransform::new(
            2.0,
            ScreenPoint::new(10_000.0, 10_000.0),
            crop(),
            viewport(),
        );
        assert_eq!(view.pan(), ScreenPoint::new(0.0, 0.0));
        let view = ViewTransform::new(
            2.0,
            ScreenPoint::new(-10_000.0, -10_000.0),
            crop(),
            viewport(),
        );
        assert_eq!(
            view.pan(),
            ScreenPoint::new(1000.0 - 1600.0, 800.0 - 1200.0)
        );
    }

    #[test]
    fn smaller_images_are_centred() {
        let view = ViewTransform::new(0.5, ScreenPoint::new(0.0, 0.0), crop(), viewport());
        assert_eq!(view.pan(), ScreenPoint::new(300.0, 250.0));
    }

    #[test]
    fn visible_rect_is_clipped_to_crop() {
        let view = ViewTransform::fit(crop(), viewport());
        let visible = view.visible_doc_rect().unwrap();
        assert_eq!(visible, crop());
    }

    #[test]
    fn zoom_is_clamped() {
        let view = ViewTransform::fit(crop(), viewport());
        assert_eq!(
            view.zoomed_at(ScreenPoint::new(0.0, 0.0), 1000.0).zoom(),
            MAX_ZOOM
        );
        assert_eq!(
            view.zoomed_at(ScreenPoint::new(0.0, 0.0), 0.0001).zoom(),
            MIN_ZOOM
        );
    }
}
