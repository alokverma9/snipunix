//! Frozen-image selection overlay.
//!
//! M0 prototype: one fullscreen, undecorated window per monitor. Every window
//! shows the same frozen desktop texture, offset so each monitor displays its
//! own slice. A shared [`Selection`] lives in virtual-desktop coordinates.
//!
//! Deliberately out of scope for M0: move/resize handles, magnifier, toolbars,
//! countdown, window snapping, keyboard nudging and monitor-hotplug handling.

#![forbid(unsafe_code)]

mod widget;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use snipunix_core::{
    CaptureMapping, DesktopLayout, ImageRect, LogicalPoint, LogicalRect, Monitor, Selection,
    SelectionPhase,
};
use thiserror::Error;

use widget::SelectionWidget;

/// One monitor that will host an overlay surface.
#[derive(Clone)]
pub struct OverlayMonitor {
    pub gdk: gdk::Monitor,
    pub geometry: Monitor,
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
    pub(crate) texture: gdk::Texture,
    pub(crate) mapping: CaptureMapping,
    pub(crate) desktop: LogicalRect,
    pub(crate) monitors: Vec<Monitor>,
    pub(crate) selection: RefCell<Selection>,
    pub(crate) pointer: Cell<Option<LogicalPoint>>,
    pub(crate) windows: RefCell<Vec<gtk::Window>>,
    pub(crate) widgets: RefCell<Vec<SelectionWidget>>,
    finished: Cell<bool>,
    on_done: RefCell<Option<DoneCallback>>,
}

impl OverlayState {
    fn queue_redraw(&self) {
        for widget in self.widgets.borrow().iter() {
            widget.queue_draw();
        }
    }
}

/// Presents the frozen-image selection overlay.
///
/// `on_done` is called exactly once, after every overlay window has been
/// destroyed. It runs on the GTK main thread.
pub fn present<F>(
    app: &impl IsA<gtk::Application>,
    texture: gdk::Texture,
    mapping: CaptureMapping,
    monitors: Vec<OverlayMonitor>,
    on_done: F,
) -> Result<(), OverlayError>
where
    F: FnOnce(OverlayResult) + 'static,
{
    if monitors.is_empty() {
        return Err(OverlayError::NoMonitors);
    }
    let desktop = mapping.desktop();
    let state = Rc::new(OverlayState {
        texture,
        mapping,
        desktop,
        monitors: monitors.iter().map(|m| m.geometry.clone()).collect(),
        selection: RefCell::new(Selection::new(desktop)),
        pointer: Cell::new(None),
        windows: RefCell::new(Vec::new()),
        widgets: RefCell::new(Vec::new()),
        finished: Cell::new(false),
        on_done: RefCell::new(Some(Box::new(on_done))),
    });

    for (index, overlay_monitor) in monitors.iter().enumerate() {
        let window = gtk::Window::builder()
            .application(app)
            .decorated(false)
            .resizable(false)
            .title("Snipunix region selection")
            .build();
        window.set_can_focus(true);

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

    // Present every surface before handing control to the user, then focus the
    // primary (first) monitor so Escape and Enter always have an owner.
    for window in state.windows.borrow().iter() {
        window.present();
    }
    if let Some(widget) = state.widgets.borrow().first() {
        widget.grab_focus();
    }
    tracing::debug!(
        monitors = state.monitors.len(),
        desktop = ?desktop,
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
        tracing::debug!(?phase, n_press, x, y, "overlay: primary press");
        if n_press == 2 && phase == SelectionPhase::Selected {
            commit(&press_state);
            return;
        }
        if phase == SelectionPhase::Selected {
            let inside = press_state
                .selection
                .borrow()
                .rect()
                .is_some_and(|rect| rect.contains_point(point));
            if inside {
                // M0 does not move selections; keep the existing rectangle so
                // double-click confirmation remains possible.
                return;
            }
        }
        {
            let mut selection = press_state.selection.borrow_mut();
            selection.begin(point);
        }
        press_state.queue_redraw();
    });

    let release_state = Rc::clone(state);
    click.connect_released(move |gesture, _n_press, x, y| {
        if gesture.current_button() != gdk::BUTTON_PRIMARY {
            return;
        }
        if release_state.selection.borrow().phase() != SelectionPhase::Dragging {
            return;
        }
        let point = virtual_point(&release_state, index, x, y);
        let accepted = {
            let mut selection = release_state.selection.borrow_mut();
            selection.finish(point)
        };
        release_state.pointer.set(Some(point));
        if accepted {
            tracing::debug!(x, y, "overlay: primary release, capturing selection");
            commit(&release_state);
            return;
        }
        tracing::debug!(
            x,
            y,
            "overlay: primary release rejected (selection too small)"
        );
        release_state.queue_redraw();
    });
    widget.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    let motion_state = Rc::clone(state);
    motion.connect_motion(move |_controller, x, y| {
        let point = virtual_point(&motion_state, index, x, y);
        motion_state.pointer.set(Some(point));
        if motion_state.selection.borrow().phase() == SelectionPhase::Dragging {
            motion_state.selection.borrow_mut().update(point);
        }
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
    keys.connect_key_pressed(
        move |_controller, keyval, _keycode, _modifiers| match keyval {
            gdk::Key::Escape => {
                let phase = key_state.selection.borrow().phase();
                tracing::debug!(?phase, "overlay: Escape pressed");
                match phase {
                    SelectionPhase::Idle => finish(&key_state, OverlayResult::Cancelled),
                    SelectionPhase::Dragging | SelectionPhase::Selected => {
                        key_state.selection.borrow_mut().clear();
                        key_state.queue_redraw();
                    }
                }
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let phase = key_state.selection.borrow().phase();
                tracing::debug!(?phase, "overlay: Enter pressed");
                if phase == SelectionPhase::Selected {
                    commit(&key_state);
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        },
    );
    window.add_controller(keys);
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
