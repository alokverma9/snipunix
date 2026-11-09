//! Geometry in canonical captured-image coordinates.
//!
//! Every annotation is stored in the coordinate space of the *original*
//! captured image. Crop is a view over that image, so annotations never move
//! when the crop changes. Screen coordinates, zoom and pan live in
//! [`crate::view`] and are never stored in the document.

/// A point in original captured-image pixels. Coordinates may be fractional.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DocPoint {
    pub x: f64,
    pub y: f64,
}

impl DocPoint {
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    #[must_use]
    pub fn midpoint(self, other: Self) -> Self {
        Self::new((self.x + other.x) / 2.0, (self.y + other.y) / 2.0)
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// A size in image pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DocSize {
    pub width: f64,
    pub height: f64,
}

impl DocSize {
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub fn is_positive(self) -> bool {
        self.width > 0.0 && self.height > 0.0 && self.width.is_finite() && self.height.is_finite()
    }
}

/// An axis-aligned rectangle in original captured-image pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DocRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl DocRect {
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
    pub fn from_corners(a: DocPoint, b: DocPoint) -> Self {
        Self::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (b.x - a.x).abs(),
            (b.y - a.y).abs(),
        )
    }

    #[must_use]
    pub fn from_size(origin: DocPoint, size: DocSize) -> Self {
        Self::new(origin.x, origin.y, size.width, size.height)
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
    pub fn origin(self) -> DocPoint {
        DocPoint::new(self.x, self.y)
    }

    #[must_use]
    pub fn size(self) -> DocSize {
        DocSize::new(self.width, self.height)
    }

    #[must_use]
    pub fn center(self) -> DocPoint {
        DocPoint::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
            || !self.x.is_finite()
            || !self.y.is_finite()
            || !self.width.is_finite()
            || !self.height.is_finite()
    }

    #[must_use]
    pub fn contains(self, point: DocPoint) -> bool {
        point.x >= self.x && point.y >= self.y && point.x < self.right() && point.y < self.bottom()
    }

    #[must_use]
    pub fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right > x && bottom > y {
            Some(Self::new(x, y, right - x, bottom - y))
        } else {
            None
        }
    }

    #[must_use]
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::new(x, y, right - x, bottom - y)
    }

    #[must_use]
    pub fn translate(self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    #[must_use]
    pub fn expand(self, amount: f64) -> Self {
        Self::new(
            self.x - amount,
            self.y - amount,
            self.width + amount * 2.0,
            self.height + amount * 2.0,
        )
    }

    #[must_use]
    pub fn clamp_point(self, point: DocPoint) -> DocPoint {
        DocPoint::new(
            point.x.clamp(self.x, self.right()),
            point.y.clamp(self.y, self.bottom()),
        )
    }

    /// Maps this rectangle through a resize from `from` to `to`.
    #[must_use]
    pub fn remap(self, from: Self, to: Self) -> Self {
        if from.width <= f64::EPSILON || from.height <= f64::EPSILON {
            return self;
        }
        let sx = to.width / from.width;
        let sy = to.height / from.height;
        Self::new(
            to.x + (self.x - from.x) * sx,
            to.y + (self.y - from.y) * sy,
            self.width * sx,
            self.height * sy,
        )
    }

    /// Rounds outward to whole image pixels, clamped to `bounds`.
    #[must_use]
    pub fn to_pixel_rect(self, bounds: Self) -> Option<snipunix_core::ImageRect> {
        let clipped = self.intersection(bounds)?;
        let left = clipped.x.floor().clamp(0.0, bounds.width) as u32;
        let top = clipped.y.floor().clamp(0.0, bounds.height) as u32;
        let right = clipped.right().ceil().clamp(0.0, bounds.width) as u32;
        let bottom = clipped.bottom().ceil().clamp(0.0, bounds.height) as u32;
        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);
        (width > 0 && height > 0).then(|| snipunix_core::ImageRect::new(left, top, width, height))
    }

    #[must_use]
    pub fn to_tiny_skia(self) -> tiny_skia::Rect {
        tiny_skia::Rect::from_xywh(
            self.x as f32,
            self.y as f32,
            self.width as f32,
            self.height as f32,
        )
        .unwrap_or_else(|| tiny_skia::Rect::from_xywh(0.0, 0.0, 0.0, 0.0).expect("zero rect"))
    }

    /// Smallest rectangle containing every point.
    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = DocPoint>) -> Option<Self> {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut any = false;
        for point in points {
            if !point.is_finite() {
                continue;
            }
            any = true;
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
        any.then(|| Self::new(min_x, min_y, max_x - min_x, max_y - min_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_normalised() {
        let a = DocPoint::new(90.0, 80.0);
        let b = DocPoint::new(10.0, 20.0);
        assert_eq!(
            DocRect::from_corners(a, b),
            DocRect::new(10.0, 20.0, 80.0, 60.0)
        );
    }

    #[test]
    fn remap_scales_relative_geometry() {
        let from = DocRect::new(10.0, 20.0, 100.0, 50.0);
        let to = DocRect::new(0.0, 0.0, 200.0, 100.0);
        let inner = DocRect::new(35.0, 30.0, 20.0, 10.0);
        assert_eq!(inner.remap(from, to), DocRect::new(50.0, 20.0, 40.0, 20.0));
    }

    #[test]
    fn pixel_rect_rounds_outward_and_clips() {
        let bounds = DocRect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(
            DocRect::new(10.25, 20.75, 30.1, 10.2).to_pixel_rect(bounds),
            Some(snipunix_core::ImageRect::new(10, 20, 31, 11))
        );
        assert_eq!(
            DocRect::new(90.0, 90.0, 50.0, 50.0).to_pixel_rect(bounds),
            Some(snipunix_core::ImageRect::new(90, 90, 10, 10))
        );
        assert_eq!(
            DocRect::new(200.0, 200.0, 10.0, 10.0).to_pixel_rect(bounds),
            None
        );
    }

    #[test]
    fn intersection_and_union_are_stable() {
        let a = DocRect::new(0.0, 0.0, 10.0, 10.0);
        let b = DocRect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(b), Some(DocRect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.union(b), DocRect::new(0.0, 0.0, 15.0, 15.0));
    }
}
