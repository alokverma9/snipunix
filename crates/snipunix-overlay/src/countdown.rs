//! Delay countdown shown before a capture.
//!
//! The countdown is an ordinary fullscreen window on the primary monitor. It is
//! destroyed before the portal request so the countdown UI cannot appear in the
//! screenshot.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use glib::subclass::prelude::*;
use gtk::graphene;
use gtk::pango;
use gtk::prelude::*;
use snipunix_core::LogicalPoint;

use crate::OverlayError;

const DIM: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.55);
const ACCENT: gdk::RGBA = gdk::RGBA::new(0.208, 0.518, 0.894, 1.0);
const TRACK: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 0.18);
const TEXT: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);

mod imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct CountdownWidget {
        /// Remaining fraction of the countdown, 1.0 down to 0.0.
        pub(crate) fraction: Cell<f64>,
        /// Numeral to display (seconds remaining, rounded up).
        pub(crate) remaining: Cell<u32>,
        pub(crate) width: Cell<i32>,
        pub(crate) height: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CountdownWidget {
        const NAME: &'static str = "SnipunixCountdownWidget";
        type Type = super::CountdownWidget;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CountdownWidget {}

    impl WidgetImpl for CountdownWidget {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => {
                    let width = self.width.get().max(1);
                    (width, width, -1, -1)
                }
                _ => {
                    let height = self.height.get().max(1);
                    (height, height, -1, -1)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let width = f64::from(widget.width().max(1));
            let height = f64::from(widget.height().max(1));
            snapshot.append_color(
                &DIM,
                &graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
            );

            let center = LogicalPoint::new(width / 2.0, height / 2.0);
            let radius = 96.0;
            draw_ring(snapshot, center, radius, self.fraction.get());

            let text = self.remaining.get().to_string();
            let layout = widget.create_pango_layout(Some(&text));
            layout.set_font_description(Some(&pango::FontDescription::from_string("Sans 64")));
            let (text_width, text_height) = layout.pixel_size();
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (center.x - f64::from(text_width) / 2.0) as f32,
                (center.y - f64::from(text_height) / 2.0) as f32,
            ));
            snapshot.append_layout(&layout, &TEXT);
            snapshot.restore();

            let hint = "Click or press Esc to cancel";
            let hint_layout = widget.create_pango_layout(Some(hint));
            hint_layout.set_font_description(Some(&pango::FontDescription::from_string("Sans 11")));
            let (hint_width, _hint_height) = hint_layout.pixel_size();
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (center.x - f64::from(hint_width) / 2.0) as f32,
                (center.y + radius + 40.0) as f32,
            ));
            snapshot.append_layout(&hint_layout, &gdk::RGBA::new(1.0, 1.0, 1.0, 0.75));
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    pub struct CountdownWidget(ObjectSubclass<imp::CountdownWidget>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CountdownWidget {
    fn new(total_seconds: u32, width: i32, height: i32) -> Self {
        let widget: Self = glib::Object::new();
        widget.imp().fraction.set(1.0);
        widget.imp().remaining.set(total_seconds.max(1));
        widget.imp().width.set(width);
        widget.imp().height.set(height);
        widget
    }

    fn update(&self, remaining: f64, total: f64) {
        self.imp()
            .fraction
            .set((remaining / total.max(1.0)).clamp(0.0, 1.0));
        self.imp().remaining.set((remaining.ceil() as u32).max(1));
        self.queue_draw();
    }
}

fn draw_ring(snapshot: &gtk::Snapshot, center: LogicalPoint, radius: f64, fraction: f64) {
    const SEGMENTS: u32 = 72;
    const DOT: f64 = 7.0;
    let active = (f64::from(SEGMENTS) * fraction).ceil() as u32;
    for index in 0..SEGMENTS {
        // Place each segment directly on the circle instead of relying on
        // snapshot transforms, which are easy to get wrong across renderers.
        let angle = std::f64::consts::TAU * f64::from(index) / f64::from(SEGMENTS);
        let x = center.x + radius * angle.cos() - DOT / 2.0;
        let y = center.y + radius * angle.sin() - DOT / 2.0;
        let color = if index < active { ACCENT } else { TRACK };
        snapshot.append_color(
            &color,
            &graphene::Rect::new(x as f32, y as f32, DOT as f32, DOT as f32),
        );
    }
}

/// Runs a `seconds`-long countdown on the primary monitor.
///
/// `on_done(true)` runs after the countdown completes; `on_done(false)` runs
/// when the user cancels. The countdown window is destroyed before the callback
/// and a short delay gives the compositor a frame to remove it.
pub fn start_countdown<F>(
    app: &impl IsA<gtk::Application>,
    seconds: u32,
    on_done: F,
) -> Result<(), OverlayError>
where
    F: FnOnce(bool) + 'static,
{
    if seconds == 0 {
        on_done(true);
        return Ok(());
    }
    let display = gdk::Display::default().ok_or(OverlayError::NoDisplay)?;
    let monitors = display.monitors();
    let monitor = monitors
        .item(0)
        .and_downcast::<gdk::Monitor>()
        .ok_or(OverlayError::NoMonitors)?;

    let window = gtk::Window::builder()
        .application(app)
        .decorated(false)
        .resizable(false)
        .title("Snipunix countdown")
        .build();
    // GTK windows are opaque by default; the countdown must let the desktop
    // show through its dim layer.
    let provider = gtk::CssProvider::new();
    provider.load_from_string("window.snipunix-countdown { background-color: transparent; }");
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    window.add_css_class("snipunix-countdown");
    window.set_opacity(0.0);
    let geometry = monitor.geometry();
    let widget = CountdownWidget::new(seconds, geometry.width(), geometry.height());
    widget.set_hexpand(true);
    widget.set_vexpand(true);
    window.set_child(Some(&widget));

    let click = gtk::GestureClick::new();
    click.connect_pressed({
        let window = window.clone();
        move |_gesture, _n_press, _x, _y| {
            tracing::debug!("countdown: click, cancelling");
            window.close();
        }
    });
    widget.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let window = window.clone();
        move |_controller, keyval, _keycode, _modifiers| {
            if keyval == gdk::Key::Escape {
                tracing::debug!("countdown: Escape, cancelling");
                window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    window.fullscreen_on_monitor(&monitor);
    window.present();
    widget.grab_focus();
    let reveal_window = window.clone();
    glib::idle_add_local_once(move || {
        reveal_window.set_opacity(1.0);
    });

    let completed = Rc::new(Cell::new(false));
    let callback = Rc::new(RefCell::new(Some(
        Box::new(on_done) as Box<dyn FnOnce(bool)>
    )));

    let finish: Rc<dyn Fn(bool)> = {
        let window = window.clone();
        let completed = Rc::clone(&completed);
        let callback = Rc::clone(&callback);
        Rc::new(move |success: bool| {
            if completed.replace(true) {
                return;
            }
            tracing::debug!(success, "countdown finished");
            window.destroy();
            let callback = callback.borrow_mut().take();
            if let Some(callback) = callback {
                // Let the compositor remove the countdown surface before the
                // portal captures the desktop.
                glib::timeout_add_local_once(Duration::from_millis(160), move || {
                    callback(success);
                });
            }
        })
    };

    // Cancel when the user closes the window.
    {
        let finish = Rc::clone(&finish);
        window.connect_close_request(move |_window| {
            finish(false);
            glib::Propagation::Stop
        });
    }

    let start = Instant::now();
    let total = f64::from(seconds);
    let finish_tick = Rc::clone(&finish);
    let widget_tick = widget.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        if completed.get() {
            return glib::ControlFlow::Break;
        }
        let elapsed = start.elapsed().as_secs_f64();
        let remaining = (total - elapsed).max(0.0);
        widget_tick.update(remaining, total);
        if remaining <= 0.0 {
            finish_tick(true);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });

    Ok(())
}
