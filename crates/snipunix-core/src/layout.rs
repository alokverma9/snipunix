//! Monitor layout in virtual-desktop logical coordinates.

use thiserror::Error;

use crate::geometry::{LogicalPoint, LogicalRect, LogicalSize};

/// One monitor as reported by the display server.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    /// Stable identifier for the lifetime of one capture (usually the GDK
    /// monitor index).
    pub id: u32,
    pub name: String,
    /// Top-left corner in virtual-desktop logical coordinates.
    pub origin: LogicalPoint,
    /// Size in logical pixels.
    pub size: LogicalSize,
    /// Scale factor reported by the toolkit (1.0 = no scaling).
    pub scale: f64,
}

impl Monitor {
    #[must_use]
    pub fn rect(&self) -> LogicalRect {
        LogicalRect::new(
            self.origin.x,
            self.origin.y,
            self.size.width,
            self.size.height,
        )
    }

    #[must_use]
    pub fn local_to_virtual(&self, p: LogicalPoint) -> LogicalPoint {
        LogicalPoint::new(self.origin.x + p.x, self.origin.y + p.y)
    }

    #[must_use]
    pub fn virtual_to_local(&self, p: LogicalPoint) -> LogicalPoint {
        LogicalPoint::new(p.x - self.origin.x, p.y - self.origin.y)
    }
}

/// All monitors that participate in one capture.
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopLayout {
    monitors: Vec<Monitor>,
}

impl DesktopLayout {
    /// Builds a layout, rejecting empty or invalid monitor lists.
    pub fn new(monitors: Vec<Monitor>) -> Result<Self, LayoutError> {
        if monitors.is_empty() {
            return Err(LayoutError::NoMonitors);
        }
        for monitor in &monitors {
            if !monitor.size.is_positive() {
                return Err(LayoutError::InvalidSize {
                    id: monitor.id,
                    width: monitor.size.width,
                    height: monitor.size.height,
                });
            }
            if !monitor.scale.is_finite() || monitor.scale <= 0.0 {
                return Err(LayoutError::InvalidScale {
                    id: monitor.id,
                    scale: monitor.scale,
                });
            }
        }
        Ok(Self { monitors })
    }

    #[must_use]
    pub fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    /// Bounding box of every monitor, guaranteed non-empty for a valid layout.
    #[must_use]
    pub fn bounding_box(&self) -> LogicalRect {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for monitor in &self.monitors {
            let rect = monitor.rect();
            min_x = min_x.min(rect.x);
            min_y = min_y.min(rect.y);
            max_x = max_x.max(rect.right());
            max_y = max_y.max(rect.bottom());
        }
        LogicalRect::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }

    /// Returns the first monitor containing a virtual-desktop point.
    #[must_use]
    pub fn monitor_at(&self, point: LogicalPoint) -> Option<&Monitor> {
        self.monitors
            .iter()
            .find(|monitor| monitor.rect().contains_point(point))
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum LayoutError {
    #[error("no monitors were reported by the display server")]
    NoMonitors,
    #[error("monitor {id} has an invalid size: {width}x{height}")]
    InvalidSize { id: u32, width: f64, height: f64 },
    #[error("monitor {id} has an invalid scale factor: {scale}")]
    InvalidScale { id: u32, scale: f64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: u32, x: f64, y: f64, w: f64, h: f64, scale: f64) -> Monitor {
        Monitor {
            id,
            name: format!("monitor-{id}"),
            origin: LogicalPoint::new(x, y),
            size: LogicalSize::new(w, h),
            scale,
        }
    }

    #[test]
    fn bounding_box_handles_negative_origins() {
        let layout = DesktopLayout::new(vec![
            monitor(0, 0.0, 0.0, 1920.0, 1080.0, 1.0),
            monitor(1, -1280.0, 0.0, 1280.0, 1024.0, 1.25),
        ])
        .expect("valid layout");

        assert_eq!(
            layout.bounding_box(),
            LogicalRect::new(-1280.0, 0.0, 3200.0, 1080.0)
        );
    }

    #[test]
    fn rejects_invalid_layouts() {
        assert_eq!(DesktopLayout::new(Vec::new()), Err(LayoutError::NoMonitors));
        assert!(matches!(
            DesktopLayout::new(vec![monitor(0, 0.0, 0.0, 0.0, 1080.0, 1.0)]),
            Err(LayoutError::InvalidSize { .. })
        ));
        assert!(matches!(
            DesktopLayout::new(vec![monitor(0, 0.0, 0.0, 1920.0, 1080.0, 0.0)]),
            Err(LayoutError::InvalidScale { .. })
        ));
    }

    #[test]
    fn converts_between_local_and_virtual_coordinates() {
        let m = monitor(1, 1920.0, 0.0, 1280.0, 1024.0, 2.0);
        let local = LogicalPoint::new(100.0, 50.0);
        let virtual_point = m.local_to_virtual(local);
        assert_eq!(virtual_point, LogicalPoint::new(2020.0, 50.0));
        assert_eq!(m.virtual_to_local(virtual_point), local);
    }
}
