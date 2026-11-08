//! Rectangular rubber-band selection state machine.

use crate::geometry::{LogicalPoint, LogicalRect};

/// Smallest selection that is accepted as a real rectangle, in logical pixels.
pub const MIN_SELECTION_SIZE: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionPhase {
    /// No anchor and no rectangle.
    Idle,
    /// Pointer is down and the rectangle follows the pointer.
    Dragging,
    /// Pointer was released and the rectangle is accepted.
    Selected,
}

/// Selection state shared by every overlay window.
///
/// The stored rectangle always lives in virtual-desktop logical coordinates.
/// Input points are clamped to the captured desktop before being stored, so a
/// drag that leaves the monitor cannot produce an out-of-bounds crop.
#[derive(Debug, Clone)]
pub struct Selection {
    bounds: LogicalRect,
    phase: SelectionPhase,
    anchor: Option<LogicalPoint>,
    rect: Option<LogicalRect>,
}

impl Selection {
    #[must_use]
    pub fn new(bounds: LogicalRect) -> Self {
        Self {
            bounds,
            phase: SelectionPhase::Idle,
            anchor: None,
            rect: None,
        }
    }

    #[must_use]
    pub fn phase(&self) -> SelectionPhase {
        self.phase
    }

    #[must_use]
    pub fn rect(&self) -> Option<LogicalRect> {
        self.rect
    }

    #[must_use]
    pub fn bounds(&self) -> LogicalRect {
        self.bounds
    }

    /// Starts a new selection at a virtual-desktop point.
    pub fn begin(&mut self, point: LogicalPoint) {
        let point = self.bounds.clamp_point(point);
        self.anchor = Some(point);
        self.rect = Some(LogicalRect::new(point.x, point.y, 0.0, 0.0));
        self.phase = SelectionPhase::Dragging;
    }

    /// Updates the rectangle while the pointer is down.
    pub fn update(&mut self, point: LogicalPoint) {
        if self.phase != SelectionPhase::Dragging {
            return;
        }
        let Some(anchor) = self.anchor else {
            return;
        };
        let point = self.bounds.clamp_point(point);
        self.rect = Some(LogicalRect::from_corners(anchor, point));
    }

    /// Finishes the drag. Returns `true` when a usable rectangle was selected.
    pub fn finish(&mut self, point: LogicalPoint) -> bool {
        if self.phase != SelectionPhase::Dragging {
            return self.phase == SelectionPhase::Selected;
        }
        self.update(point);
        let accepted = self.rect.is_some_and(|rect| {
            rect.width >= MIN_SELECTION_SIZE && rect.height >= MIN_SELECTION_SIZE
        });
        if accepted {
            self.phase = SelectionPhase::Selected;
        } else {
            self.phase = SelectionPhase::Idle;
            self.rect = None;
        }
        self.anchor = None;
        accepted
    }

    /// Clears the selection and returns to the idle state.
    pub fn clear(&mut self) {
        self.phase = SelectionPhase::Idle;
        self.anchor = None;
        self.rect = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> Selection {
        Selection::new(LogicalRect::new(0.0, 0.0, 1920.0, 1080.0))
    }

    #[test]
    fn drag_in_any_direction_produces_a_normalised_rect() {
        let mut s = selection();
        s.begin(LogicalPoint::new(500.0, 400.0));
        assert!(s.finish(LogicalPoint::new(100.0, 150.0)));
        assert_eq!(s.phase(), SelectionPhase::Selected);
        assert_eq!(s.rect(), Some(LogicalRect::new(100.0, 150.0, 400.0, 250.0)));
    }

    #[test]
    fn tiny_drag_is_rejected() {
        let mut s = selection();
        s.begin(LogicalPoint::new(500.0, 400.0));
        assert!(!s.finish(LogicalPoint::new(505.0, 403.0)));
        assert_eq!(s.phase(), SelectionPhase::Idle);
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn drag_is_clamped_to_desktop_bounds() {
        let mut s = selection();
        s.begin(LogicalPoint::new(1900.0, 1000.0));
        assert!(s.finish(LogicalPoint::new(5000.0, 5000.0)));
        assert_eq!(s.rect(), Some(LogicalRect::new(1900.0, 1000.0, 20.0, 80.0)));
    }

    #[test]
    fn escape_semantics_clear_then_idle() {
        let mut s = selection();
        s.begin(LogicalPoint::new(10.0, 10.0));
        assert!(s.finish(LogicalPoint::new(100.0, 100.0)));
        s.clear();
        assert_eq!(s.phase(), SelectionPhase::Idle);
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn update_ignored_when_not_dragging() {
        let mut s = selection();
        s.update(LogicalPoint::new(10.0, 10.0));
        assert_eq!(s.phase(), SelectionPhase::Idle);
        assert_eq!(s.rect(), None);
    }
}
