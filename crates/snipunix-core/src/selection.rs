//! Rectangular rubber-band selection state machine.
//!
//! The state machine deliberately lives in the core crate so every transition
//! can be tested without a display server. The overlay crate only turns input
//! events into calls on [`Selection`].

use crate::geometry::{LogicalPoint, LogicalRect};

/// Smallest selection that is accepted as a real rectangle, in logical pixels.
pub const MIN_SELECTION_SIZE: f64 = 8.0;

/// Side length of a resize handle in logical pixels.
pub const HANDLE_SIZE: f64 = 10.0;

/// Extra click tolerance around a handle, in logical pixels.
pub const HANDLE_HIT_SLOP: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionPhase {
    /// No anchor and no rectangle.
    Idle,
    /// Pointer is down and a new rectangle follows the pointer.
    Dragging,
    /// Pointer was released and the rectangle is accepted.
    Selected,
    /// Pointer is down inside an accepted rectangle.
    Moving,
    /// Pointer is down on one of the eight resize handles.
    Resizing,
}

impl SelectionPhase {
    /// `true` while the pointer owns the selection.
    #[must_use]
    pub fn is_drag(self) -> bool {
        matches!(
            self,
            SelectionPhase::Dragging | SelectionPhase::Moving | SelectionPhase::Resizing
        )
    }
}

/// One of the eight resize handles around a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
}

impl Handle {
    pub const ALL: [Self; 8] = [
        Self::NorthWest,
        Self::North,
        Self::NorthEast,
        Self::East,
        Self::SouthEast,
        Self::South,
        Self::SouthWest,
        Self::West,
    ];

    /// Centre of this handle for `rect`, in the same coordinate space.
    #[must_use]
    pub fn center(self, rect: LogicalRect) -> LogicalPoint {
        let left = rect.x;
        let center_x = rect.x + rect.width / 2.0;
        let right = rect.right();
        let top = rect.y;
        let center_y = rect.y + rect.height / 2.0;
        let bottom = rect.bottom();
        match self {
            Self::NorthWest => LogicalPoint::new(left, top),
            Self::North => LogicalPoint::new(center_x, top),
            Self::NorthEast => LogicalPoint::new(right, top),
            Self::East => LogicalPoint::new(right, center_y),
            Self::SouthEast => LogicalPoint::new(right, bottom),
            Self::South => LogicalPoint::new(center_x, bottom),
            Self::SouthWest => LogicalPoint::new(left, bottom),
            Self::West => LogicalPoint::new(left, center_y),
        }
    }

    /// Returns the handle under `point`, corners before edges.
    #[must_use]
    pub fn hit(rect: LogicalRect, point: LogicalPoint, slop: f64) -> Option<Self> {
        let on_x = |x: f64| (point.x - x).abs() <= slop;
        let on_y = |y: f64| (point.y - y).abs() <= slop;
        let left = rect.x;
        let right = rect.right();
        let top = rect.y;
        let bottom = rect.bottom();
        let middle_x = rect.x + rect.width / 2.0;
        let middle_y = rect.y + rect.height / 2.0;

        if on_x(left) && on_y(top) {
            return Some(Self::NorthWest);
        }
        if on_x(right) && on_y(top) {
            return Some(Self::NorthEast);
        }
        if on_x(right) && on_y(bottom) {
            return Some(Self::SouthEast);
        }
        if on_x(left) && on_y(bottom) {
            return Some(Self::SouthWest);
        }
        if on_y(top) && (point.x - middle_x).abs() <= slop {
            return Some(Self::North);
        }
        if on_x(right) && (point.y - middle_y).abs() <= slop {
            return Some(Self::East);
        }
        if on_y(bottom) && (point.x - middle_x).abs() <= slop {
            return Some(Self::South);
        }
        if on_x(left) && (point.y - middle_y).abs() <= slop {
            return Some(Self::West);
        }
        None
    }

    /// GDK cursor name for this handle.
    #[must_use]
    pub fn cursor_name(self) -> &'static str {
        match self {
            Self::NorthWest | Self::SouthEast => "nwse-resize",
            Self::NorthEast | Self::SouthWest => "nesw-resize",
            Self::North | Self::South => "ns-resize",
            Self::East | Self::West => "ew-resize",
        }
    }

    /// `(left, right, top, bottom)` edges this handle drags.
    #[must_use]
    fn edges(self) -> (bool, bool, bool, bool) {
        match self {
            Self::NorthWest => (true, false, true, false),
            Self::North => (false, false, true, false),
            Self::NorthEast => (false, true, true, false),
            Self::East => (false, true, false, false),
            Self::SouthEast => (false, true, false, true),
            Self::South => (false, false, false, true),
            Self::SouthWest => (true, false, false, true),
            Self::West => (true, false, false, false),
        }
    }
}

/// What a pointer press did to the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressTarget {
    /// Started a new rubber-band selection.
    NewSelection,
    /// Started moving an existing selection.
    Move,
    /// Started resizing an existing selection.
    Resize(Handle),
}

/// What a pointer release did to the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// A new valid rectangle was selected.
    Accepted,
    /// An existing rectangle was moved or resized and remains selected.
    Adjusted,
    /// The gesture was too small or there was nothing to release.
    Rejected,
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
    active_handle: Option<Handle>,
    drag_origin_rect: Option<LogicalRect>,
    drag_origin_point: Option<LogicalPoint>,
}

impl Selection {
    #[must_use]
    pub fn new(bounds: LogicalRect) -> Self {
        Self {
            bounds,
            phase: SelectionPhase::Idle,
            anchor: None,
            rect: None,
            active_handle: None,
            drag_origin_rect: None,
            drag_origin_point: None,
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

    #[must_use]
    pub fn active_handle(&self) -> Option<Handle> {
        self.active_handle
    }

    /// Starts a new selection at a virtual-desktop point, discarding any
    /// previous rectangle.
    pub fn begin(&mut self, point: LogicalPoint) {
        let point = self.bounds.clamp_point(point);
        self.anchor = Some(point);
        self.rect = Some(LogicalRect::new(point.x, point.y, 0.0, 0.0));
        self.phase = SelectionPhase::Dragging;
        self.active_handle = None;
        self.drag_origin_rect = None;
        self.drag_origin_point = None;
    }

    /// Routes a pointer press to a new selection, a move, or a resize.
    ///
    /// Pressing outside an existing rectangle starts a new selection, matching
    /// the rubber-band convention.
    pub fn press(&mut self, point: LogicalPoint) -> PressTarget {
        let point = self.bounds.clamp_point(point);
        if let Some(rect) = self.rect {
            if matches!(
                self.phase,
                SelectionPhase::Selected | SelectionPhase::Moving | SelectionPhase::Resizing
            ) {
                if let Some(handle) = Handle::hit(rect, point, HANDLE_HIT_SLOP) {
                    self.phase = SelectionPhase::Resizing;
                    self.active_handle = Some(handle);
                    self.drag_origin_rect = Some(rect);
                    self.drag_origin_point = Some(point);
                    return PressTarget::Resize(handle);
                }
                if rect.contains_point(point) {
                    self.phase = SelectionPhase::Moving;
                    self.active_handle = None;
                    self.drag_origin_rect = Some(rect);
                    self.drag_origin_point = Some(point);
                    return PressTarget::Move;
                }
            }
        }
        self.begin(point);
        PressTarget::NewSelection
    }

    /// Updates the rectangle while the pointer is down.
    pub fn drag(&mut self, point: LogicalPoint) {
        let point = self.bounds.clamp_point(point);
        match self.phase {
            SelectionPhase::Dragging => {
                if let Some(anchor) = self.anchor {
                    self.rect = Some(LogicalRect::from_corners(anchor, point));
                }
            }
            SelectionPhase::Moving => {
                if let (Some(origin), Some(anchor)) =
                    (self.drag_origin_rect, self.drag_origin_point)
                {
                    let dx = point.x - anchor.x;
                    let dy = point.y - anchor.y;
                    self.rect = Some(clamp_inside(self.bounds, origin.translate(dx, dy)));
                }
            }
            SelectionPhase::Resizing => {
                if let (Some(origin), Some(handle)) = (self.drag_origin_rect, self.active_handle) {
                    self.rect = Some(resize_rect(self.bounds, origin, handle, point));
                }
            }
            SelectionPhase::Idle | SelectionPhase::Selected => {}
        }
    }

    /// Compatibility wrapper used by the M0 tests and defensive code.
    pub fn update(&mut self, point: LogicalPoint) {
        self.drag(point);
    }

    /// Finishes the gesture and reports what happened.
    pub fn release(&mut self, point: LogicalPoint) -> ReleaseOutcome {
        let outcome = match self.phase {
            SelectionPhase::Dragging => {
                self.drag(point);
                if self.rect.is_some_and(is_large_enough) {
                    self.phase = SelectionPhase::Selected;
                    ReleaseOutcome::Accepted
                } else {
                    self.clear();
                    ReleaseOutcome::Rejected
                }
            }
            SelectionPhase::Moving | SelectionPhase::Resizing => {
                self.drag(point);
                if self.rect.is_some_and(is_large_enough) {
                    self.phase = SelectionPhase::Selected;
                    ReleaseOutcome::Adjusted
                } else {
                    self.clear();
                    ReleaseOutcome::Rejected
                }
            }
            SelectionPhase::Selected => ReleaseOutcome::Accepted,
            SelectionPhase::Idle => ReleaseOutcome::Rejected,
        };
        self.anchor = None;
        self.active_handle = None;
        self.drag_origin_rect = None;
        self.drag_origin_point = None;
        outcome
    }

    /// Finishes a drag. Returns `true` when a usable rectangle remains.
    pub fn finish(&mut self, point: LogicalPoint) -> bool {
        matches!(
            self.release(point),
            ReleaseOutcome::Accepted | ReleaseOutcome::Adjusted
        )
    }

    /// Moves an accepted selection by logical pixels, clamped to the desktop.
    pub fn nudge(&mut self, dx: f64, dy: f64) -> bool {
        if self.phase != SelectionPhase::Selected {
            return false;
        }
        let Some(rect) = self.rect else {
            return false;
        };
        let moved = clamp_inside(self.bounds, rect.translate(dx, dy));
        if moved == rect {
            return false;
        }
        self.rect = Some(moved);
        true
    }

    /// Restores an accepted selection, used when adjusting a finished capture.
    pub fn select_rect(&mut self, rect: LogicalRect) {
        let rect = rect.intersection(self.bounds);
        match rect {
            Some(rect) if is_large_enough(rect) => {
                self.rect = Some(rect);
                self.phase = SelectionPhase::Selected;
                self.active_handle = None;
                self.anchor = None;
                self.drag_origin_rect = None;
                self.drag_origin_point = None;
            }
            _ => self.clear(),
        }
    }

    /// Clears the selection and returns to the idle state.
    pub fn clear(&mut self) {
        self.phase = SelectionPhase::Idle;
        self.anchor = None;
        self.rect = None;
        self.active_handle = None;
        self.drag_origin_rect = None;
        self.drag_origin_point = None;
    }
}

fn is_large_enough(rect: LogicalRect) -> bool {
    rect.width >= MIN_SELECTION_SIZE && rect.height >= MIN_SELECTION_SIZE
}

/// Translates are clamped so the whole rectangle stays inside `bounds`.
fn clamp_inside(bounds: LogicalRect, rect: LogicalRect) -> LogicalRect {
    let x = if rect.width >= bounds.width {
        bounds.x
    } else {
        rect.x.clamp(bounds.x, bounds.right() - rect.width)
    };
    let y = if rect.height >= bounds.height {
        bounds.y
    } else {
        rect.y.clamp(bounds.y, bounds.bottom() - rect.height)
    };
    LogicalRect::new(x, y, rect.width, rect.height)
}

/// Moves the edges owned by `handle`, keeping the opposite edges fixed and
/// enforcing the minimum size.
fn resize_rect(
    bounds: LogicalRect,
    origin: LogicalRect,
    handle: Handle,
    point: LogicalPoint,
) -> LogicalRect {
    let point = bounds.clamp_point(point);
    let (drag_left, drag_right, drag_top, drag_bottom) = handle.edges();

    let mut left = origin.x;
    let mut right = origin.right();
    let mut top = origin.y;
    let mut bottom = origin.bottom();

    if drag_left {
        left = point.x.min(right - MIN_SELECTION_SIZE);
    }
    if drag_right {
        right = point.x.max(left + MIN_SELECTION_SIZE);
    }
    if drag_top {
        top = point.y.min(bottom - MIN_SELECTION_SIZE);
    }
    if drag_bottom {
        bottom = point.y.max(top + MIN_SELECTION_SIZE);
    }

    // Keep both edges inside the desktop without collapsing the minimum size.
    left = left.clamp(bounds.x, bounds.right() - MIN_SELECTION_SIZE);
    right = right.clamp(bounds.x + MIN_SELECTION_SIZE, bounds.right());
    top = top.clamp(bounds.y, bounds.bottom() - MIN_SELECTION_SIZE);
    bottom = bottom.clamp(bounds.y + MIN_SELECTION_SIZE, bounds.bottom());

    // Ensure the fixed (non-dragged) edges never escape bounds even when the
    // origin rect was already slightly outside due to floating-point drift or
    // a prior resize that accumulated rounding error.
    let rect = LogicalRect::from_corners(
        LogicalPoint::new(left, top),
        LogicalPoint::new(right, bottom),
    );
    clamp_inside(bounds, rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> Selection {
        Selection::new(LogicalRect::new(0.0, 0.0, 1920.0, 1080.0))
    }

    fn selected() -> Selection {
        let mut s = selection();
        s.press(LogicalPoint::new(100.0, 100.0));
        assert_eq!(
            s.release(LogicalPoint::new(400.0, 300.0)),
            ReleaseOutcome::Accepted
        );
        s
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
        let mut s = selected();
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

    #[test]
    fn press_inside_moves_selection() {
        let mut s = selected();
        assert_eq!(s.press(LogicalPoint::new(200.0, 200.0)), PressTarget::Move);
        assert_eq!(s.phase(), SelectionPhase::Moving);
        s.drag(LogicalPoint::new(250.0, 230.0));
        assert_eq!(
            s.release(LogicalPoint::new(250.0, 230.0)),
            ReleaseOutcome::Adjusted
        );
        assert_eq!(s.rect(), Some(LogicalRect::new(150.0, 130.0, 300.0, 200.0)));
    }

    #[test]
    fn moving_is_clamped_to_desktop_bounds() {
        let mut s = selected();
        s.press(LogicalPoint::new(200.0, 200.0));
        s.release(LogicalPoint::new(5000.0, 5000.0));
        assert_eq!(
            s.rect(),
            Some(LogicalRect::new(1620.0, 880.0, 300.0, 200.0))
        );
    }

    #[test]
    fn each_handle_resizes_the_expected_edges() {
        let cases = [
            (
                Handle::NorthWest,
                50.0,
                60.0,
                LogicalRect::new(50.0, 60.0, 350.0, 240.0),
            ),
            (
                Handle::North,
                100.0,
                60.0,
                LogicalRect::new(100.0, 60.0, 300.0, 240.0),
            ),
            (
                Handle::NorthEast,
                450.0,
                60.0,
                LogicalRect::new(100.0, 60.0, 350.0, 240.0),
            ),
            (
                Handle::East,
                450.0,
                200.0,
                LogicalRect::new(100.0, 100.0, 350.0, 200.0),
            ),
            (
                Handle::SouthEast,
                450.0,
                400.0,
                LogicalRect::new(100.0, 100.0, 350.0, 300.0),
            ),
            (
                Handle::South,
                200.0,
                400.0,
                LogicalRect::new(100.0, 100.0, 300.0, 300.0),
            ),
            (
                Handle::SouthWest,
                50.0,
                400.0,
                LogicalRect::new(50.0, 100.0, 350.0, 300.0),
            ),
            (
                Handle::West,
                50.0,
                200.0,
                LogicalRect::new(50.0, 100.0, 350.0, 200.0),
            ),
        ];
        for (handle, x, y, expected) in cases {
            let mut s = selected();
            let point = handle.center(LogicalRect::new(100.0, 100.0, 300.0, 200.0));
            assert_eq!(s.press(point), PressTarget::Resize(handle));
            s.release(LogicalPoint::new(x, y));
            assert_eq!(s.rect(), Some(expected), "handle {handle:?}");
        }
    }

    #[test]
    fn resize_never_shrinks_below_the_minimum() {
        let mut s = selected();
        let point = Handle::SouthEast.center(LogicalRect::new(100.0, 100.0, 300.0, 200.0));
        s.press(point);
        s.release(LogicalPoint::new(105.0, 105.0));
        let rect = s.rect().unwrap();
        assert!(rect.width >= MIN_SELECTION_SIZE);
        assert!(rect.height >= MIN_SELECTION_SIZE);
    }

    #[test]
    fn click_outside_starts_a_new_selection() {
        let mut s = selected();
        assert_eq!(
            s.press(LogicalPoint::new(900.0, 900.0)),
            PressTarget::NewSelection
        );
        assert_eq!(s.phase(), SelectionPhase::Dragging);
    }

    #[test]
    fn nudge_moves_only_a_selected_rectangle() {
        let mut s = selected();
        assert!(s.nudge(10.0, -10.0));
        assert_eq!(s.rect(), Some(LogicalRect::new(110.0, 90.0, 300.0, 200.0)));
        assert!(!s.nudge(0.0, 0.0));
        assert!(s.nudge(1_000_000.0, 0.0));
        assert_eq!(s.rect().unwrap().right(), 1920.0);
        assert!(!s.nudge(1.0, 0.0));
    }

    #[test]
    fn select_rect_restores_an_adjustable_selection() {
        let mut s = selection();
        s.select_rect(LogicalRect::new(200.0, 150.0, 640.0, 480.0));
        assert_eq!(s.phase(), SelectionPhase::Selected);
        assert_eq!(s.rect(), Some(LogicalRect::new(200.0, 150.0, 640.0, 480.0)));
        assert_eq!(
            s.press(Handle::East.center(LogicalRect::new(200.0, 150.0, 640.0, 480.0))),
            PressTarget::Resize(Handle::East)
        );
    }

    #[test]
    fn select_rect_clips_to_desktop_and_rejects_degenerate_rects() {
        let mut s = selection();
        s.select_rect(LogicalRect::new(1800.0, 1000.0, 400.0, 400.0));
        assert_eq!(
            s.rect(),
            Some(LogicalRect::new(1800.0, 1000.0, 120.0, 80.0))
        );
        s.select_rect(LogicalRect::new(10.0, 10.0, 4.0, 400.0));
        assert_eq!(s.phase(), SelectionPhase::Idle);
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn escape_after_clear_is_a_no_op() {
        let mut s = selected();
        s.clear();
        s.clear();
        assert_eq!(s.phase(), SelectionPhase::Idle);
        assert!(!s.nudge(1.0, 1.0));
    }

    #[test]
    fn handle_hit_prefers_corners_and_respects_slop() {
        let rect = LogicalRect::new(100.0, 100.0, 300.0, 200.0);
        assert_eq!(
            Handle::hit(rect, LogicalPoint::new(102.0, 98.0), 5.0),
            Some(Handle::NorthWest)
        );
        assert_eq!(
            Handle::hit(rect, LogicalPoint::new(250.0, 302.0), 5.0),
            Some(Handle::South)
        );
        assert_eq!(
            Handle::hit(rect, LogicalPoint::new(250.0, 200.0), 5.0),
            None
        );
    }
}
