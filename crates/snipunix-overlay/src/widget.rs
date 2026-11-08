//! Custom GSK widget that renders the frozen image, dim layer and selection.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glib::subclass::prelude::*;
use gtk::graphene;
use gtk::pango;
use gtk::prelude::*;
use snipunix_core::{LogicalPoint, LogicalRect, SelectionPhase};

use crate::OverlayState;

const DIM: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.45);
const ACCENT: gdk::RGBA = gdk::RGBA::new(0.208, 0.518, 0.894, 1.0);
const TEXT: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 0.95);
const CROSSHAIR: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 0.5);
const PILL_BG: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.62);

mod imp {
    use super::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct SelectionWidget {
        pub(crate) state: RefCell<Option<Rc<OverlayState>>>,
        pub(crate) index: Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SelectionWidget {
        const NAME: &'static str = "SnipunixSelectionWidget";
        type Type = super::SelectionWidget;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SelectionWidget {}

    impl WidgetImpl for SelectionWidget {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = self
                .state
                .borrow()
                .as_ref()
                .and_then(|state| state.monitors.get(self.index.get()).cloned())
                .map_or((1, 1), |monitor| {
                    (
                        monitor.size.width.round() as i32,
                        monitor.size.height.round() as i32,
                    )
                });
            match orientation {
                gtk::Orientation::Horizontal => (size.0, size.0, -1, -1),
                _ => (size.1, size.1, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(state) = self.state.borrow().clone() else {
                return;
            };
            let Some(monitor) = state.monitors.get(self.index.get()).cloned() else {
                return;
            };

            let widget = self.obj();
            let width = f64::from(widget.width().max(1));
            let height = f64::from(widget.height().max(1));

            // Frozen image: draw the whole captured desktop scaled into logical
            // pixels, offset so this monitor shows its own slice.
            let (logical_scale_x, logical_scale_y) = state.mapping.logical_scale();
            let image = state.mapping.image();
            let image_width = f64::from(image.width) * logical_scale_x;
            let image_height = f64::from(image.height) * logical_scale_y;
            let image_origin = graphene::Rect::new(
                (state.desktop.x - monitor.origin.x) as f32,
                (state.desktop.y - monitor.origin.y) as f32,
                image_width as f32,
                image_height as f32,
            );
            snapshot.append_texture(&state.texture, &image_origin);

            // Dim everything, then redraw the undimmed frozen image inside the
            // selection hole.
            snapshot.append_color(
                &DIM,
                &graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
            );

            let selection = state.selection.borrow().rect();
            if let Some(selection) = selection {
                let local = LogicalRect::new(
                    selection.x - monitor.origin.x,
                    selection.y - monitor.origin.y,
                    selection.width,
                    selection.height,
                );
                let monitor_rect = LogicalRect::new(0.0, 0.0, width, height);
                if let Some(visible) = local.intersection(monitor_rect) {
                    snapshot.push_clip(&to_graphene(visible));
                    snapshot.append_texture(&state.texture, &image_origin);
                    snapshot.pop();
                }
                draw_border(snapshot, local);
                if monitor
                    .rect()
                    .contains_point(LogicalPoint::new(selection.x, selection.y))
                {
                    draw_badge(&widget, snapshot, &state, selection, local);
                }
            }

            if let Some(pointer) = state.pointer.get() {
                if state.selection.borrow().phase() == SelectionPhase::Dragging {
                    draw_crosshair(
                        snapshot,
                        LogicalPoint::new(
                            pointer.x - monitor.origin.x,
                            pointer.y - monitor.origin.y,
                        ),
                        width,
                        height,
                    );
                }
            }

            if self.index.get() == 0 {
                let phase = state.selection.borrow().phase();
                let hint = match phase {
                    SelectionPhase::Idle => "Drag to select  ·  Esc to cancel",
                    SelectionPhase::Dragging => "Release to capture  ·  Esc to cancel",
                    SelectionPhase::Selected => "Capturing selection…",
                };
                draw_pill(&widget, snapshot, hint, width as f32 / 2.0, 24.0);
            }
        }
    }
}

glib::wrapper! {
    pub struct SelectionWidget(ObjectSubclass<imp::SelectionWidget>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SelectionWidget {
    pub(crate) fn new(state: &Rc<OverlayState>, index: usize) -> Self {
        let widget: Self = glib::Object::new();
        widget.imp().state.replace(Some(Rc::clone(state)));
        widget.imp().index.set(index);
        widget
    }
}

fn to_graphene(rect: LogicalRect) -> graphene::Rect {
    graphene::Rect::new(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    )
}

fn draw_border(snapshot: &gtk::Snapshot, rect: LogicalRect) {
    const THICKNESS: f64 = 2.0;
    snapshot.append_color(
        &ACCENT,
        &to_graphene(LogicalRect::new(rect.x, rect.y, rect.width, THICKNESS)),
    );
    snapshot.append_color(
        &ACCENT,
        &to_graphene(LogicalRect::new(
            rect.x,
            rect.bottom() - THICKNESS,
            rect.width,
            THICKNESS,
        )),
    );
    snapshot.append_color(
        &ACCENT,
        &to_graphene(LogicalRect::new(rect.x, rect.y, THICKNESS, rect.height)),
    );
    snapshot.append_color(
        &ACCENT,
        &to_graphene(LogicalRect::new(
            rect.right() - THICKNESS,
            rect.y,
            THICKNESS,
            rect.height,
        )),
    );
}

fn draw_crosshair(snapshot: &gtk::Snapshot, point: LogicalPoint, width: f64, height: f64) {
    if point.x < 0.0 || point.y < 0.0 || point.x > width || point.y > height {
        return;
    }
    snapshot.append_color(
        &CROSSHAIR,
        &graphene::Rect::new(0.0, point.y as f32, width as f32, 1.0),
    );
    snapshot.append_color(
        &CROSSHAIR,
        &graphene::Rect::new(point.x as f32, 0.0, 1.0, height as f32),
    );
}

fn draw_badge(
    widget: &SelectionWidget,
    snapshot: &gtk::Snapshot,
    state: &OverlayState,
    selection: LogicalRect,
    local: LogicalRect,
) {
    let Some(image) = state.mapping.virtual_to_image(selection) else {
        return;
    };
    let text = format!("{} × {}", image.width, image.height);
    let layout = widget.create_pango_layout(Some(&text));
    layout.set_font_description(Some(&pango::FontDescription::from_string("Monospace 11")));
    let (text_width, text_height) = layout.pixel_size();
    let x = local.x + 8.0;
    let y = local.y + 8.0;
    let background = graphene::Rect::new(
        x as f32,
        y as f32,
        text_width as f32 + 16.0,
        text_height as f32 + 8.0,
    );
    snapshot.append_color(&PILL_BG, &background);
    snapshot.save();
    snapshot.translate(&graphene::Point::new(x as f32 + 8.0, y as f32 + 4.0));
    snapshot.append_layout(&layout, &TEXT);
    snapshot.restore();
}

fn draw_pill(
    widget: &SelectionWidget,
    snapshot: &gtk::Snapshot,
    text: &str,
    center_x: f32,
    y: f32,
) {
    let layout = widget.create_pango_layout(Some(text));
    layout.set_font_description(Some(&pango::FontDescription::from_string("Sans 11")));
    let (text_width, text_height) = layout.pixel_size();
    let width = text_width as f32 + 20.0;
    let height = text_height as f32 + 10.0;
    let x = center_x - width / 2.0;
    let background = graphene::Rect::new(x, y, width, height);
    snapshot.append_color(&PILL_BG, &background);
    snapshot.save();
    snapshot.translate(&graphene::Point::new(x + 10.0, y + 5.0));
    snapshot.append_layout(&layout, &TEXT);
    snapshot.restore();
}
