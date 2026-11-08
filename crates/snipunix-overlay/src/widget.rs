//! Custom GSK widget that renders the frozen image, dim layer, selection,
//! resize handles and magnifier.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use glib::subclass::prelude::*;
use gtk::graphene;
use gtk::pango;
use gtk::prelude::*;
use snipunix_core::{HANDLE_SIZE, Handle, LogicalPoint, LogicalRect, SelectionPhase};

use crate::OverlayState;

const DIM: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.40);
const ACCENT: gdk::RGBA = gdk::RGBA::new(0.208, 0.518, 0.894, 1.0);
const TEXT: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 0.95);
const CROSSHAIR: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 0.5);
const PILL_BG: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.62);
const HANDLE_FILL: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
const BORDER_LIGHT: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, 0.30);

const MAGNIFIER_SIZE: f64 = 140.0;
const MAGNIFIER_ZOOM: f64 = 8.0;
const MAGNIFIER_OFFSET: f64 = 24.0;

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

            // Frozen image. A pre-cropped monitor texture fills the monitor
            // exactly; the full texture is the fallback when cropping failed.
            let image_origin = draw_frozen(snapshot, &state, &monitor, width, height);

            // Dim everything, then redraw the undimmed frozen image inside the
            // selection hole.
            snapshot.append_color(
                &DIM,
                &graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
            );

            let phase = state.selection.borrow().phase();
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
                    draw_frozen(snapshot, &state, &monitor, width, height);
                    snapshot.pop();
                }
                draw_border(snapshot, local);
                if matches!(
                    phase,
                    SelectionPhase::Selected | SelectionPhase::Moving | SelectionPhase::Resizing
                ) {
                    draw_handles(snapshot, local);
                }
                if local.intersection(monitor_rect).is_some() {
                    draw_badge(&widget, snapshot, &state, selection, local, width, height);
                }
            }

            if let Some(pointer) = state.pointer.get() {
                if phase.is_drag() {
                    let local_pointer = LogicalPoint::new(
                        pointer.x - monitor.origin.x,
                        pointer.y - monitor.origin.y,
                    );
                    draw_crosshair(snapshot, local_pointer, width, height);
                    if state.magnifier
                        && matches!(phase, SelectionPhase::Dragging | SelectionPhase::Resizing)
                    {
                        draw_magnifier(
                            snapshot,
                            &state,
                            &image_origin,
                            local_pointer,
                            width,
                            height,
                        );
                    }
                }
            }

            if self.index.get() == 0 {
                let hint = match phase {
                    SelectionPhase::Idle => "Drag to select  ·  Esc to cancel",
                    SelectionPhase::Dragging => "Release to capture  ·  Esc to cancel",
                    SelectionPhase::Selected if state.auto_commit => "Copying selection…",
                    SelectionPhase::Selected
                    | SelectionPhase::Moving
                    | SelectionPhase::Resizing => {
                        "Drag handles to adjust  ·  Enter or Ctrl+C to copy  ·  Esc to clear"
                    }
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

/// Draws the frozen slice and returns the rectangle the full texture occupies
/// in this widget, for the magnifier.
fn draw_frozen(
    snapshot: &gtk::Snapshot,
    state: &OverlayState,
    monitor: &snipunix_core::Monitor,
    width: f64,
    height: f64,
) -> graphene::Rect {
    let index = state.monitors.iter().position(|m| m.id == monitor.id);
    let uses_slice = index.is_some_and(|index| state.has_monitor_frame(index));
    if let Some(index) = index.filter(|_| uses_slice) {
        let texture = state.frame_for(index);
        let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
        snapshot.append_texture(&texture, &bounds);
        return bounds;
    }

    // Fallback: the whole desktop, offset so this monitor shows its slice.
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
    snapshot.append_texture(&state.frames.full, &image_origin);
    image_origin
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
    // Outer accent border.
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
    // Thin light inner border for contrast on light content.
    snapshot.append_color(
        &BORDER_LIGHT,
        &to_graphene(LogicalRect::new(
            rect.x + THICKNESS,
            rect.y + THICKNESS,
            rect.width - 2.0 * THICKNESS,
            1.0,
        )),
    );
}

fn draw_handles(snapshot: &gtk::Snapshot, rect: LogicalRect) {
    for handle in Handle::ALL {
        let center = handle.center(rect);
        let left = center.x - HANDLE_SIZE / 2.0;
        let top = center.y - HANDLE_SIZE / 2.0;
        let square = LogicalRect::new(left, top, HANDLE_SIZE, HANDLE_SIZE);
        snapshot.append_color(&HANDLE_FILL, &to_graphene(square));
        draw_outline(snapshot, square, &ACCENT, 1.0);
    }
}

fn draw_outline(snapshot: &gtk::Snapshot, rect: LogicalRect, color: &gdk::RGBA, width: f64) {
    snapshot.append_color(
        color,
        &to_graphene(LogicalRect::new(rect.x, rect.y, rect.width, width)),
    );
    snapshot.append_color(
        color,
        &to_graphene(LogicalRect::new(
            rect.x,
            rect.bottom() - width,
            rect.width,
            width,
        )),
    );
    snapshot.append_color(
        color,
        &to_graphene(LogicalRect::new(rect.x, rect.y, width, rect.height)),
    );
    snapshot.append_color(
        color,
        &to_graphene(LogicalRect::new(
            rect.right() - width,
            rect.y,
            width,
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
    monitor_width: f64,
    monitor_height: f64,
) {
    let Some(image) = state.mapping.virtual_to_image(selection) else {
        return;
    };
    let text = format!("{} × {}", image.width, image.height);
    let layout = widget.create_pango_layout(Some(&text));
    layout.set_font_description(Some(&pango::FontDescription::from_string("Monospace 11")));
    let (text_width, text_height) = layout.pixel_size();

    let badge_width = f64::from(text_width) + 16.0;
    let badge_height = f64::from(text_height) + 8.0;
    let x = (local.x + 8.0).clamp(0.0, (monitor_width - badge_width).max(0.0));
    let y = if local.height < badge_height + 16.0 {
        (local.bottom() + 6.0).clamp(0.0, (monitor_height - badge_height).max(0.0))
    } else {
        (local.y + 8.0).clamp(0.0, (monitor_height - badge_height).max(0.0))
    };
    let background =
        graphene::Rect::new(x as f32, y as f32, badge_width as f32, badge_height as f32);
    snapshot.append_color(&PILL_BG, &background);
    snapshot.save();
    snapshot.translate(&graphene::Point::new(x as f32 + 8.0, y as f32 + 4.0));
    snapshot.append_layout(&layout, &TEXT);
    snapshot.restore();
}

fn draw_magnifier(
    snapshot: &gtk::Snapshot,
    state: &OverlayState,
    image_origin: &graphene::Rect,
    pointer: LogicalPoint,
    width: f64,
    height: f64,
) {
    if pointer.x < 0.0 || pointer.y < 0.0 || pointer.x > width || pointer.y > height {
        return;
    }
    let size = MAGNIFIER_SIZE;
    let mut x = pointer.x + MAGNIFIER_OFFSET;
    if x + size > width {
        x = pointer.x - MAGNIFIER_OFFSET - size;
    }
    x = x.clamp(0.0, (width - size).max(0.0));
    let mut y = pointer.y + MAGNIFIER_OFFSET;
    if y + size > height {
        y = pointer.y - MAGNIFIER_OFFSET - size;
    }
    y = y.clamp(0.0, (height - size).max(0.0));

    let source_size = size / MAGNIFIER_ZOOM;
    let source_x = pointer.x - source_size / 2.0;
    let source_y = pointer.y - source_size / 2.0;

    snapshot.save();
    snapshot.push_clip(&graphene::Rect::new(
        x as f32,
        y as f32,
        size as f32,
        size as f32,
    ));
    snapshot.translate(&graphene::Point::new(
        (x - source_x * MAGNIFIER_ZOOM) as f32,
        (y - source_y * MAGNIFIER_ZOOM) as f32,
    ));
    snapshot.scale(MAGNIFIER_ZOOM as f32, MAGNIFIER_ZOOM as f32);
    snapshot.append_texture(&state.frames.full, image_origin);
    snapshot.pop();
    snapshot.restore();

    draw_outline(snapshot, LogicalRect::new(x, y, size, size), &ACCENT, 2.0);
    // Centre cross inside the magnifier.
    let center_x = x + size / 2.0;
    let center_y = y + size / 2.0;
    snapshot.append_color(
        &CROSSHAIR,
        &graphene::Rect::new(x as f32, center_y as f32, size as f32, 1.0),
    );
    snapshot.append_color(
        &CROSSHAIR,
        &graphene::Rect::new(center_x as f32, y as f32, 1.0, size as f32),
    );
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
