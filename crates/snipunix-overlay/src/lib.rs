//! Frozen-image selection overlay.
//!
//! One fullscreen, undecorated window per monitor. Each window shows its slice
//! of the frozen desktop. A shared [`Selection`] lives in virtual-desktop
//! coordinates, so pointer input from any monitor updates the same rectangle.
//!
//! M1 adds move/resize handles, a magnifier, keyboard nudging, monitor-hotplug
//! cancellation and an "adjust existing selection" mode. Annotation, toolbars
//! and window snapping remain later milestones.

#![forbid(unsafe_code)]

mod countdown;
mod widget;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use snipunix_core::{
    CaptureMapping, DesktopLayout, HANDLE_HIT_SLOP, Handle, ImageRect, LogicalPoint, LogicalRect,
    Monitor, ReleaseOutcome, Selection, SelectionPhase,
};
use thiserror::Error;

pub use countdown::start_countdown;
use widget::SelectionWidget;

/// One monitor that will host an overlay surface.
#[derive(Clone)]
pub struct OverlayMonitor {
    pub gdk: gdk::Monitor,
    pub geometry: Monitor,
}

/// Textures prepared by the app.
///
/// `full` is used by the magnifier. `per_monitor` holds a pre-cropped texture
/// for each monitor (or `None` when cropping failed and the full texture must
/// be used as a fallback).
pub struct OverlayFrames {
    pub full: gdk::Texture,
    pub per_monitor: Vec<Option<gdk::Texture>>,
}

/// Behaviour of one overlay session.
#[derive(Debug, Clone, Copy)]
pub struct OverlayOptions {
    /// Commit as soon as a new drag is released. This is the default Snipping
    /// Tool-style flow requested for Snipunix.
    pub auto_commit_on_release: bool,
    /// Restore an existing rectangle and wait for an explicit confirmation.
    pub initial_selection: Option<LogicalRect>,
    /// Show the pixel magnifier while dragging or resizing.
    pub magnifier: bool,
    /// Opacity of the dim layer outside the selection, in `0.0..=1.0`.
    pub dim_strength: f32,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            auto_commit_on_release: true,
            initial_selection: None,
            magnifier: true,
            dim_strength: 0.40,
        }
    }
}

/// Final outcome of one overlay session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayResult {
    Selected {
        /// Selection in virtual-desktop logical coordinates.
        logical: LogicalRect,
        /// Selection converted to captured-image pixels.
        image: ImageRect,
    },
    Cancelled,
}

#[derive(Debug, Error)]
pub enum OverlayError {
    #[error("no GDK display is available")]
    NoDisplay,
    #[error("the display reported no monitors")]
    NoMonitors,
    #[error("display monitor {0} disappeared while building the overlay")]
    MonitorUnavailable(u32),
    #[error("invalid monitor layout: {0}")]
    InvalidLayout(#[from] snipunix_core::LayoutError),
    #[error("the texture list does not match the monitor count")]
    TextureCountMismatch,
}

/// Reads the current monitor layout from GDK.
pub fn collect_monitors() -> Result<Vec<OverlayMonitor>, OverlayError> {
    let display = gdk::Display::default().ok_or(OverlayError::NoDisplay)?;
    let monitors = display.monitors();
    let mut out = Vec::with_capacity(monitors.n_items() as usize);
    for index in 0..monitors.n_items() {
        let gdk_monitor = monitors
            .item(index)
            .and_downcast::<gdk::Monitor>()
            .ok_or(OverlayError::MonitorUnavailable(index))?;
        let geometry = gdk_monitor.geometry();
        out.push(OverlayMonitor {
            gdk: gdk_monitor.clone(),
            geometry: Monitor {
                id: index,
                name: gdk_monitor
                    .connector()
                    .map_or_else(|| format!("monitor-{index}"), |name| name.to_string()),
                origin: LogicalPoint::new(f64::from(geometry.x()), f64::from(geometry.y())),
                size: snipunix_core::LogicalSize::new(
                    f64::from(geometry.width()),
                    f64::from(geometry.height()),
                ),
                scale: f64::from(gdk_monitor.scale_factor()),
            },
        });
    }
    if out.is_empty() {
        return Err(OverlayError::NoMonitors);
    }
    Ok(out)
}

type DoneCallback = Box<dyn FnOnce(OverlayResult)>;

pub(crate) struct OverlayState {
    pub(crate) frames: OverlayFrames,
    pub(crate) mapping: CaptureMapping,
    pub(crate) desktop: LogicalRect,
    pub(crate) monitors: Vec<Monitor>,
    pub(crate) selection: RefCell<Selection>,
    pub(crate) pointer: Cell<Option<LogicalPoint>>,
    pub(crate) windows: RefCell<Vec<gtk::Window>>,
    pub(crate) widgets: RefCell<Vec<SelectionWidget>>,
    pub(crate) auto_commit: bool,
    pub(crate) magnifier: bool,
    pub(crate) dim_strength: f32,
    finished: Cell<bool>,
    on_done: RefCell<Option<DoneCallback>>,
    monitor_handler: RefCell<Option<(gio::ListModel, glib::SignalHandlerId)>>,
}

impl OverlayState {
    fn queue_redraw(&self) {
        for widget in self.widgets.borrow().iter() {
            widget.queue_draw();
        }
    }

    pub(crate) fn frame_for(&self, index: usize) -> gdk::Texture {
        self.frames
            .per_monitor
            .get(index)
            .and_then(Clone::clone)
            .unwrap_or_else(|| self.frames.full.clone())
    }

    pub(crate) fn has_monitor_frame(&self, index: usize) -> bool {
        self.frames
            .per_monitor
            .get(index)
            .is_some_and(Option::is_some)
    }
}

/// Presents the frozen-image selection overlay.
///
/// `on_done` is called exactly once, after every overlay window has been
/// destroyed. It runs on the GTK main thread.
pub fn present<F>(
    app: &impl IsA<gtk::Application>,
    frames: OverlayFrames,
    mapping: CaptureMapping,
    monitors: Vec<OverlayMonitor>,
    options: OverlayOptions,
    on_done: F,
) -> Result<(), OverlayError>
where
    F: FnOnce(OverlayResult) + 'static,
{
    if monitors.is_empty() {
        return Err(OverlayError::NoMonitors);
    }
    if frames.per_monitor.len() != monitors.len() {
        return Err(OverlayError::TextureCountMismatch);
    }
    let desktop = mapping.desktop();
    let mut selection = Selection::new(desktop);
    if let Some(initial) = options.initial_selection {
        selection.select_rect(initial);
    }
    let state = Rc::new(OverlayState {
        frames,
        mapping,
        desktop,
        monitors: monitors.iter().map(|m| m.geometry.clone()).collect(),
        selection: RefCell::new(selection),
        pointer: Cell::new(None),
        windows: RefCell::new(Vec::new()),
        widgets: RefCell::new(Vec::new()),
        auto_commit: options.auto_commit_on_release,
        magnifier: options.magnifier,
        dim_strength: options.dim_strength.clamp(0.0, 1.0),
        finished: Cell::new(false),
        on_done: RefCell::new(Some(Box::new(on_done))),
        monitor_handler: RefCell::new(None),
    });

    for (index, overlay_monitor) in monitors.iter().enumerate() {
        let window = gtk::Window::builder()
            .application(app)
            .decorated(false)
            .resizable(false)
            .title("Snipunix region selection")
            .build();
        window.set_can_focus(true);
        // Realize fully transparent, then reveal in a single frame so the
        // compositor does not animate a half-drawn overlay.
        window.set_opacity(0.0);

        let widget = SelectionWidget::new(&state, index);
        widget.set_focusable(true);
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.set_cursor_from_name(Some("crosshair"));
        window.set_child(Some(&widget));

        install_input(&window, &widget, &state, index);
        install_keyboard(&window, &state);

        window.fullscreen_on_monitor(&overlay_monitor.gdk);
        state.windows.borrow_mut().push(window);
        state.widgets.borrow_mut().push(widget);
    }

    // Present every surface before handing control to the user.
    for window in state.windows.borrow().iter() {
        window.present();
    }
    if let Some(widget) = state.widgets.borrow().first() {
        widget.grab_focus();
    }

    // Monitor hotplug invalidates every frozen slice and every crop; cancel.
    if let Some(display) = gdk::Display::default() {
        let hotplug_state = Rc::clone(&state);
        let monitors_model = display.monitors();
        let handler =
            monitors_model.connect_items_changed(move |_model, _position, _removed, _added| {
                tracing::debug!("overlay: monitor layout changed, cancelling");
                finish(&hotplug_state, OverlayResult::Cancelled);
            });
        state
            .monitor_handler
            .replace(Some((monitors_model, handler)));
    }

    let reveal_state = Rc::clone(&state);
    glib::idle_add_local_once(move || {
        if reveal_state.finished.get() {
            return;
        }
        for window in reveal_state.windows.borrow().iter() {
            window.set_opacity(1.0);
        }
    });

    tracing::debug!(
        monitors = state.monitors.len(),
        desktop = ?desktop,
        auto_commit = state.auto_commit,
        "selection overlay presented"
    );
    Ok(())
}

fn install_input(
    window: &gtk::Window,
    widget: &SelectionWidget,
    state: &Rc<OverlayState>,
    index: usize,
) {
    let click = gtk::GestureClick::new();
    click.set_button(0); // listen to every button, then filter
    let press_state = Rc::clone(state);
    let press_widget = widget.clone();
    click.connect_pressed(move |gesture, n_press, x, y| {
        let point = virtual_point(&press_state, index, x, y);
        press_state.pointer.set(Some(point));

        if gesture.current_button() == gdk::BUTTON_SECONDARY {
            tracing::debug!("overlay: secondary click, cancelling");
            finish(&press_state, OverlayResult::Cancelled);
            return;
        }
        if gesture.current_button() != gdk::BUTTON_PRIMARY {
            return;
        }

        let phase = press_state.selection.borrow().phase();
        if n_press == 2 && phase == SelectionPhase::Selected {
            tracing::debug!("overlay: double-click, committing");
            commit(&press_state);
            return;
        }

        let target = press_state.selection.borrow_mut().press(point);
        tracing::debug!(?phase, ?target, x, y, "overlay: primary press");
        update_cursor(&press_widget, &press_state, point);
        press_state.queue_redraw();
    });

    let release_state = Rc::clone(state);
    let release_widget = widget.clone();
    click.connect_released(move |gesture, _n_press, x, y| {
        if gesture.current_button() != gdk::BUTTON_PRIMARY {
            return;
        }
        let phase = release_state.selection.borrow().phase();
        if !phase.is_drag() {
            return;
        }
        let point = virtual_point(&release_state, index, x, y);
        let outcome = release_state.selection.borrow_mut().release(point);
        release_state.pointer.set(Some(point));
        tracing::debug!(?phase, ?outcome, x, y, "overlay: primary release");

        match (phase, outcome) {
            (SelectionPhase::Dragging, ReleaseOutcome::Accepted) if release_state.auto_commit => {
                commit(&release_state);
            }
            (SelectionPhase::Dragging, ReleaseOutcome::Accepted) => {
                release_state.queue_redraw();
            }
            (_, ReleaseOutcome::Adjusted) => {
                release_state.queue_redraw();
            }
            (_, ReleaseOutcome::Rejected) => {
                release_state.queue_redraw();
            }
            _ => {}
        }
        update_cursor(&release_widget, &release_state, point);
    });
    widget.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    let motion_state = Rc::clone(state);
    let motion_widget = widget.clone();
    motion.connect_motion(move |_controller, x, y| {
        let point = virtual_point(&motion_state, index, x, y);
        motion_state.pointer.set(Some(point));
        if motion_state.selection.borrow().phase().is_drag() {
            motion_state.selection.borrow_mut().drag(point);
        }
        update_cursor(&motion_widget, &motion_state, point);
        motion_state.queue_redraw();
    });
    widget.add_controller(motion);

    let close_state = Rc::clone(state);
    window.connect_close_request(move |_window| {
        finish(&close_state, OverlayResult::Cancelled);
        glib::Propagation::Stop
    });
}

fn install_keyboard(window: &gtk::Window, state: &Rc<OverlayState>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let key_state = Rc::clone(state);
    keys.connect_key_pressed(move |_controller, keyval, _keycode, modifiers| {
        let phase = key_state.selection.borrow().phase();
        match keyval {
            gdk::Key::Escape => {
                tracing::debug!(?phase, "overlay: Escape pressed");
                match phase {
                    SelectionPhase::Idle | SelectionPhase::Dragging => {
                        finish(&key_state, OverlayResult::Cancelled);
                    }
                    SelectionPhase::Selected
                    | SelectionPhase::Moving
                    | SelectionPhase::Resizing => {
                        key_state.selection.borrow_mut().clear();
                        key_state.queue_redraw();
                    }
                }
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                tracing::debug!(?phase, "overlay: Enter pressed");
                if phase == SelectionPhase::Selected {
                    commit(&key_state);
                }
                glib::Propagation::Stop
            }
            gdk::Key::c | gdk::Key::C if modifiers.contains(gdk::ModifierType::CONTROL_MASK) => {
                tracing::debug!(?phase, "overlay: Ctrl+C pressed");
                if phase == SelectionPhase::Selected {
                    commit(&key_state);
                }
                glib::Propagation::Stop
            }
            gdk::Key::Left | gdk::Key::Right | gdk::Key::Up | gdk::Key::Down => {
                if phase != SelectionPhase::Selected {
                    return glib::Propagation::Proceed;
                }
                let step = if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                    10.0
                } else {
                    1.0
                };
                let (dx, dy) = match keyval {
                    gdk::Key::Left => (-step, 0.0),
                    gdk::Key::Right => (step, 0.0),
                    gdk::Key::Up => (0.0, -step),
                    _ => (0.0, step),
                };
                key_state.selection.borrow_mut().nudge(dx, dy);
                key_state.queue_redraw();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);
}

fn update_cursor(widget: &SelectionWidget, state: &OverlayState, point: LogicalPoint) {
    let selection = state.selection.borrow();
    if let Some(rect) = selection.rect() {
        if matches!(
            selection.phase(),
            SelectionPhase::Selected | SelectionPhase::Moving | SelectionPhase::Resizing
        ) {
            if let Some(handle) = Handle::hit(rect, point, HANDLE_HIT_SLOP) {
                widget.set_cursor_from_name(Some(handle.cursor_name()));
                return;
            }
            if rect.contains_point(point) {
                widget.set_cursor_from_name(Some("move"));
                return;
            }
        }
    }
    widget.set_cursor_from_name(Some("crosshair"));
}

fn virtual_point(state: &OverlayState, index: usize, x: f64, y: f64) -> LogicalPoint {
    let monitor = &state.monitors[index];
    monitor.local_to_virtual(LogicalPoint::new(x, y))
}

fn commit(state: &Rc<OverlayState>) {
    let Some(logical) = state.selection.borrow().rect() else {
        tracing::debug!("overlay: commit requested with no selection");
        return;
    };
    let Some(image) = state.mapping.virtual_to_image(logical) else {
        tracing::debug!(?logical, "overlay: selection does not map to the image");
        finish(state, OverlayResult::Cancelled);
        return;
    };
    tracing::debug!(?logical, ?image, "overlay: committing selection");
    finish(state, OverlayResult::Selected { logical, image });
}

fn finish(state: &Rc<OverlayState>, result: OverlayResult) {
    if state.finished.replace(true) {
        return;
    }
    tracing::debug!(?result, "overlay: finishing and destroying surfaces");
    // Invariant: every terminal state destroys every overlay surface before any
    // other work (including the completion callback).
    if let Some((model, handler)) = state.monitor_handler.borrow_mut().take() {
        model.disconnect(handler);
    }
    let windows: Vec<_> = state.windows.borrow_mut().drain(..).collect();
    for window in windows {
        window.destroy();
    }
    state.widgets.borrow_mut().clear();
    if let Some(callback) = state.on_done.borrow_mut().take() {
        callback(result);
    }
}

/// Builds a layout value for the app from collected monitors.
pub fn layout_from_monitors(monitors: &[OverlayMonitor]) -> Result<DesktopLayout, OverlayError> {
    Ok(DesktopLayout::new(
        monitors.iter().map(|m| m.geometry.clone()).collect(),
    )?)
}

/// `true` when the state machine reports a `Pressed`/drag interaction.
#[must_use]
pub fn is_dragging(state: &Selection) -> bool {
    state.phase().is_drag()
}

#[cfg(test)]
mod tests {
    use super::*;
    use snipunix_core::PressTarget;

    #[test]
    fn default_options_commit_on_release() {
        let options = OverlayOptions::default();
        assert!(options.auto_commit_on_release);
        assert!(options.initial_selection.is_none());
        assert!(options.magnifier);
        assert!((options.dim_strength - 0.40).abs() < f32::EPSILON);
    }

    #[test]
    fn press_and_release_targets_are_consistent() {
        let mut selection = Selection::new(LogicalRect::new(0.0, 0.0, 1920.0, 1080.0));
        assert_eq!(
            selection.press(LogicalPoint::new(10.0, 10.0)),
            PressTarget::NewSelection
        );
        assert_eq!(
            selection.release(LogicalPoint::new(100.0, 100.0)),
            ReleaseOutcome::Accepted
        );
    }
}
