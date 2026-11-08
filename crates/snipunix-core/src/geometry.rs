//! Primitive geometry types for logical desktop coordinates.

/// A point in virtual-desktop logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

impl LogicalPoint {
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalSize {
    pub width: f64,
    pub height: f64,
}

impl LogicalSize {
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub fn is_positive(self) -> bool {
        self.width > 0.0 && self.height > 0.0 && self.width.is_finite() && self.height.is_finite()
    }
}

/// An axis-aligned rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Builds a normalised rectangle from two arbitrary corners.
    #[must_use]
    pub fn from_corners(a: LogicalPoint, b: LogicalPoint) -> Self {
        Self {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            width: (b.x - a.x).abs(),
            height: (b.y - a.y).abs(),
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
    pub fn is_empty(self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
            || !self.x.is_finite()
            || !self.y.is_finite()
            || !self.width.is_finite()
            || !self.height.is_finite()
    }

    #[must_use]
    pub fn contains_point(self, p: LogicalPoint) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
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

    /// Clamps a point into the half-open rectangle.
    #[must_use]
    pub fn clamp_point(self, p: LogicalPoint) -> LogicalPoint {
        LogicalPoint::new(
            p.x.clamp(self.x, self.right()),
            p.y.clamp(self.y, self.bottom()),
        )
    }

    #[must_use]
    pub fn translate(self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy, self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_normalised_in_every_direction() {
        let down_right =
            LogicalRect::from_corners(LogicalPoint::new(10.0, 20.0), LogicalPoint::new(50.0, 80.0));
        let up_left =
            LogicalRect::from_corners(LogicalPoint::new(50.0, 80.0), LogicalPoint::new(10.0, 20.0));

        assert_eq!(down_right, LogicalRect::new(10.0, 20.0, 40.0, 60.0));
        assert_eq!(down_right, up_left);
    }

    #[test]
    fn clamp_point_stays_inside() {
        let bounds = LogicalRect::new(-100.0, -50.0, 200.0, 100.0);
        assert_eq!(
            bounds.clamp_point(LogicalPoint::new(-500.0, 500.0)),
            LogicalPoint::new(-100.0, 50.0)
        );
    }
}
