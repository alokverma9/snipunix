//! Non-destructive annotation editor built on GTK4 and the shared rasteriser.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::cairo;
use libadwaita as adw;
use snipunix_annotate::{
    Annotation, AnnotationId, AnnotationKind, Arrow, Color, DocPoint, DocRect, Document, Editor,
    ExportFormat, Line, MIN_CROP_SIZE, Preview, Redaction, RenderEngine, RenderedImage, Shape,
    Stroke, Text, ViewTransform, save,
};
use snipunix_capture::CapturedImage;
use snipunix_ocr::OcrResult;
use snipunix_settings::{SaveFormat, Settings};

use snipunix_annotate::view::{ScreenPoint, ScreenSize};

const HANDLE_SIZE: f64 = 8.0;
const HANDLE_PICK_RADIUS: f64 = 9.0;
const MIN_SHAPE_SIZE: f64 = 2.0;
const DEFAULT_TEXT_WIDTH: f64 = 320.0;
const HIGHLIGHTER_PRESETS: [(&str, f64); 3] = [("Light", 0.22), ("Medium", 0.36), ("Strong", 0.55)];

/// Callbacks that connect the editor to the surrounding application shell.
#[derive(Clone)]
pub struct Callbacks {
    pub status: Rc<dyn Fn(&str)>,
    pub toast: Rc<dyn Fn(&str)>,
    pub close: Rc<dyn Fn()>,
    pub copied: Rc<dyn Fn()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Move,
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Eraser,
    Crop,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Self::Move => "Move / select",
            Self::Pen => "Pen",
            Self::Highlighter => "Highlighter",
            Self::Line => "Line",
            Self::Arrow => "Arrow",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Text => "Text",
            Self::Eraser => "Eraser",
            Self::Crop => "Crop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
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
    const ALL: [Self; 8] = [
        Self::NorthWest,
        Self::North,
        Self::NorthEast,
        Self::East,
        Self::SouthEast,
        Self::South,
        Self::SouthWest,
        Self::West,
    ];

    fn position(self, rect: DocRect) -> DocPoint {
        match self {
            Self::NorthWest => DocPoint::new(rect.x, rect.y),
            Self::North => DocPoint::new(rect.center().x, rect.y),
            Self::NorthEast => DocPoint::new(rect.right(), rect.y),
            Self::East => DocPoint::new(rect.right(), rect.center().y),
            Self::SouthEast => DocPoint::new(rect.right(), rect.bottom()),
            Self::South => DocPoint::new(rect.center().x, rect.bottom()),
            Self::SouthWest => DocPoint::new(rect.x, rect.bottom()),
            Self::West => DocPoint::new(rect.x, rect.center().y),
        }
    }

    fn moves_left(self) -> bool {
        matches!(self, Self::NorthWest | Self::West | Self::SouthWest)
    }

    fn moves_right(self) -> bool {
        matches!(self, Self::NorthEast | Self::East | Self::SouthEast)
    }

    fn moves_top(self) -> bool {
        matches!(self, Self::NorthWest | Self::North | Self::NorthEast)
    }

    fn moves_bottom(self) -> bool {
        matches!(self, Self::SouthWest | Self::South | Self::SouthEast)
    }
}

struct OwnedPreview {
    annotation: Annotation,
    replace: bool,
}

struct OcrDialog {
    window: adw::Window,
    buffer: gtk::TextBuffer,
    text_view: gtk::TextView,
    remove_line_breaks: gtk::Switch,
    redact: gtk::Button,
    status: gtk::Label,
}

struct OcrOverlay {
    result: OcrResult,
    /// Crop origin in original-image coordinates. OCR boxes are crop-relative.
    origin: DocPoint,
    selected: Option<usize>,
    dialog: OcrDialog,
}

enum Interaction {
    Pan {
        pan: ScreenPoint,
    },
    Stroke {
        last: DocPoint,
    },
    Shape {
        start: DocPoint,
    },
    Move {
        original: Annotation,
        start: DocPoint,
    },
    Resize {
        original: Annotation,
        handle: Handle,
    },
    Erase {
        ids: Vec<AnnotationId>,
    },
    Crop {
        start: DocPoint,
    },
}

struct Inner {
    editor: Editor,
    engine: RenderEngine,
    view: ViewTransform,
    tool: Tool,
    color: Color,
    width: f64,
    font_size: f64,
    highlighter_opacity: f64,
    export_format: ExportFormat,
    selection: Option<AnnotationId>,
    preview: Option<OwnedPreview>,
    interaction: Option<Interaction>,
    crop_candidate: Option<DocRect>,
    cursor: Option<ScreenPoint>,
    drag_start: Option<ScreenPoint>,
    drag_moved: bool,
    needs_fit: bool,
    ocr: Option<OcrOverlay>,
    ocr_busy: Cell<bool>,
    settings: Rc<RefCell<Settings>>,
    settings_path: std::path::PathBuf,
}

impl Inner {
    fn new(image: CapturedImage, settings: Rc<RefCell<Settings>>) -> Result<Self, String> {
        let document = Document::new(image).map_err(|error| error.to_string())?;
        let crop = document.crop_rect();
        let settings_path = Settings::default_path();
        let export_format = match settings.borrow().files.format {
            SaveFormat::Png => ExportFormat::Png,
            SaveFormat::Jpeg => ExportFormat::Jpeg,
            SaveFormat::Webp => ExportFormat::WebP,
        };
        Ok(Self {
            editor: Editor::new(document),
            engine: RenderEngine::new(),
            view: ViewTransform::fit(crop, ScreenSize::new(0.0, 0.0)),
            tool: Tool::Move,
            color: Color::rgb(0xE0, 0x1B, 0x24),
            width: 3.0,
            font_size: 22.0,
            highlighter_opacity: 0.36,
            export_format,
            selection: None,
            preview: None,
            interaction: None,
            crop_candidate: None,
            cursor: None,
            drag_start: None,
            drag_moved: false,
            needs_fit: true,
            ocr: None,
            ocr_busy: Cell::new(false),
            settings,
            settings_path,
        })
    }

    fn status_text(&self) -> String {
        let history = if self.editor.can_undo() || self.editor.can_redo() {
            format!(
                " · undo {}/redo {}",
                self.editor.undo_depth(),
                self.editor.redo_depth()
            )
        } else {
            String::new()
        };
        let selection = self.selection.and_then(|id| {
            self.editor
                .document()
                .annotation(id)
                .map(|annotation| format!(" · selected {:?}", annotation.id))
        });
        let crop = self
            .crop_candidate
            .map_or(String::new(), |_| " · crop ready to apply".to_string());
        format!(
            "{}{history}{}{crop}",
            self.tool.label(),
            selection.unwrap_or_default()
        )
    }
}

#[derive(Clone)]
struct Ui {
    canvas: gtk::DrawingArea,
    undo: gtk::Button,
    redo: gtk::Button,
    status: gtk::Label,
    crop_apply: gtk::Button,
    crop_cancel: gtk::Button,
    callbacks: Callbacks,
}

impl Ui {
    fn sync(&self, inner: &Rc<RefCell<Inner>>) {
        let state = inner.borrow();
        self.undo.set_sensitive(state.editor.can_undo());
        self.redo.set_sensitive(state.editor.can_redo());
        self.crop_apply
            .set_sensitive(state.crop_candidate.is_some());
        self.crop_cancel
            .set_sensitive(state.crop_candidate.is_some());
        self.status.set_label(&state.status_text());
        self.canvas.queue_draw();
    }

    fn set_status(&self, message: &str) {
        (self.callbacks.status)(message);
    }

    fn toast(&self, message: &str) {
        (self.callbacks.toast)(message);
    }
}

/// The editor page. The owning application puts [`Self::root`] in its stack.
pub struct EditorView {
    pub root: gtk::Box,
    inner: Rc<RefCell<Inner>>,
    ui: Ui,
}

impl EditorView {
    pub fn new(
        image: CapturedImage,
        parent: gtk::Window,
        callbacks: Callbacks,
        settings: Rc<RefCell<Settings>>,
    ) -> Result<Self, String> {
        let inner = Rc::new(RefCell::new(Inner::new(image, settings)?));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);

        let canvas = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .content_width(640)
            .content_height(480)
            .build();
        canvas.set_focusable(true);

        let status = gtk::Label::new(Some("Move / select"));
        status.set_xalign(0.0);
        status.add_css_class("dim-label");
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_hexpand(true);

        let undo = gtk::Button::from_icon_name("edit-undo-symbolic");
        undo.set_tooltip_text(Some("Undo (Ctrl+Z)"));
        let redo = gtk::Button::from_icon_name("edit-redo-symbolic");
        redo.set_tooltip_text(Some("Redo (Ctrl+Shift+Z)"));
        let crop_apply = gtk::Button::with_label("Apply Crop");
        let crop_cancel = gtk::Button::with_label("Cancel Crop");
        crop_apply.set_sensitive(false);
        crop_cancel.set_sensitive(false);

        let ui = Ui {
            canvas: canvas.clone(),
            undo: undo.clone(),
            redo: redo.clone(),
            status: status.clone(),
            crop_apply: crop_apply.clone(),
            crop_cancel: crop_cancel.clone(),
            callbacks: callbacks.clone(),
        };

        let tools = tool_rail(&inner, &ui);
        let properties = properties_panel(&inner, &ui);
        let bottom = bottom_bar(
            &inner,
            &ui,
            &parent,
            &undo,
            &redo,
            &crop_apply,
            &crop_cancel,
            &status,
        );

        let main = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        main.set_hexpand(true);
        main.set_vexpand(true);
        main.set_margin_top(6);
        main.set_margin_bottom(6);
        main.set_margin_start(8);
        main.set_margin_end(8);
        main.append(&tools);
        main.append(&canvas);
        main.append(&properties);
        root.append(&main);
        root.append(&bottom);

        install_canvas_input(&canvas, &inner, &ui, &parent);
        install_draw(&canvas, &inner);

        let view = Self { root, inner, ui };
        view.ui.sync(&view.inner);
        Ok(view)
    }

    pub fn copy_annotated(&self) {
        match render_annotated(&self.inner) {
            Ok(image) => {
                if let Some(display) = gdk::Display::default() {
                    let texture = texture_from_rendered(&image);
                    display.clipboard().set_texture(&texture);
                    (self.ui.callbacks.copied)();
                    let message = format!(
                        "Copied annotated {} × {} to the clipboard.",
                        image.width(),
                        image.height()
                    );
                    self.ui.set_status(&message);
                    self.ui.toast(&message);
                } else {
                    self.ui.toast("Could not access the display clipboard.");
                }
            }
            Err(error) => self.ui.toast(&error),
        }
    }

    pub fn open_export_dialog(&self) {
        let parent = match self.root.root().and_downcast::<gtk::Window>() {
            Some(window) => window,
            None => {
                self.ui.toast("Could not locate the editor window.");
                return;
            }
        };
        open_export_dialog_impl(&self.inner, &self.ui, &parent);
    }

    pub fn extract_text(&self) {
        let parent = match self.root.root().and_downcast::<gtk::Window>() {
            Some(window) => window,
            None => {
                self.ui.toast("Could not locate the editor window.");
                return;
            }
        };
        start_ocr(&self.inner, &self.ui, &parent);
    }

    pub fn undo(&self) {
        if self.inner.borrow_mut().editor.undo() {
            self.inner.borrow_mut().selection = None;
            self.ui.sync(&self.inner);
        }
    }

    pub fn redo(&self) {
        if self.inner.borrow_mut().editor.redo() {
            self.inner.borrow_mut().selection = None;
            self.ui.sync(&self.inner);
        }
    }
}

fn tool_rail(inner: &Rc<RefCell<Inner>>, ui: &Ui) -> gtk::Widget {
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 4);
    rail.set_valign(gtk::Align::Start);
    let label = gtk::Label::new(Some("Tools"));
    label.add_css_class("dim-label");
    label.set_xalign(0.0);
    rail.append(&label);

    let definitions = [
        ("Move", Tool::Move, "edit-select-all-symbolic"),
        ("Pen", Tool::Pen, "document-edit-symbolic"),
        ("Highlight", Tool::Highlighter, "marker-symbolic"),
        ("Line", Tool::Line, "go-next-symbolic"),
        ("Arrow", Tool::Arrow, "go-up-symbolic"),
        ("Rect", Tool::Rectangle, "checkbox-symbolic"),
        ("Ellipse", Tool::Ellipse, "media-record-symbolic"),
        ("Text", Tool::Text, "insert-text-symbolic"),
        ("Eraser", Tool::Eraser, "edit-delete-symbolic"),
        ("Crop", Tool::Crop, "crop-symbolic"),
    ];

    let mut first: Option<gtk::ToggleButton> = None;
    for (name, tool, icon) in definitions {
        let button = gtk::ToggleButton::new();
        button.set_icon_name(icon);
        button.set_tooltip_text(Some(name));
        button.set_width_request(40);
        if let Some(first) = &first {
            button.set_group(Some(first));
        } else {
            first = Some(button.clone());
        }
        button.set_active(tool == Tool::Move);
        let state = Rc::clone(inner);
        let ui = ui.clone();
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            {
                let mut state = state.borrow_mut();
                state.tool = tool;
                if tool != Tool::Crop {
                    state.crop_candidate = None;
                }
            }
            ui.sync(&state);
            ui.set_status(&format!("{} selected.", tool.label()));
        });
        rail.append(&button);
    }
    rail.upcast()
}

fn properties_panel(inner: &Rc<RefCell<Inner>>, ui: &Ui) -> gtk::Widget {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
    panel.set_valign(gtk::Align::Start);
    panel.set_width_request(210);

    let color_label = gtk::Label::new(Some("Stroke colour"));
    color_label.set_xalign(0.0);
    color_label.add_css_class("dim-label");
    panel.append(&color_label);

    let palette = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    for (name, color) in snipunix_annotate::color::PRESETS {
        let button = gtk::Button::with_label(&name[..1]);
        let accessible = format!("{name} stroke colour");
        button.update_property(&[gtk::accessible::Property::Label(&accessible)]);
        button.set_tooltip_text(Some(name));
        button.set_width_request(24);
        let state = Rc::clone(inner);
        let ui = ui.clone();
        button.connect_clicked(move |_| {
            {
                let mut state = state.borrow_mut();
                state.color = color;
                apply_selection(&mut state, |annotation| annotation.with_color(color));
            }
            ui.sync(&state);
        });
        palette.append(&button);
    }
    panel.append(&palette);

    let color_dialog = gtk::ColorDialog::new();
    let color_button = gtk::ColorDialogButton::new(Some(color_dialog));
    color_button.set_rgba(&gdk::RGBA::new(0.88, 0.11, 0.14, 1.0));
    color_button.set_tooltip_text(Some("Custom stroke colour"));
    let state = Rc::clone(inner);
    let ui_for_color = ui.clone();
    color_button.connect_rgba_notify(move |button| {
        let rgba = button.rgba();
        let color = Color::rgba(
            (rgba.red() * 255.0).round() as u8,
            (rgba.green() * 255.0).round() as u8,
            (rgba.blue() * 255.0).round() as u8,
            (rgba.alpha() * 255.0).round() as u8,
        );
        {
            let mut state = state.borrow_mut();
            state.color = color;
            apply_selection(&mut state, |annotation| annotation.with_color(color));
        }
        ui_for_color.sync(&state);
    });
    panel.append(&color_button);

    let width_label = gtk::Label::new(Some("Stroke width"));
    width_label.set_xalign(0.0);
    width_label.add_css_class("dim-label");
    panel.append(&width_label);
    let width_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 40.0, 1.0);
    width_scale.set_value(3.0);
    width_scale.set_draw_value(true);
    let state = Rc::clone(inner);
    let ui_for_width = ui.clone();
    width_scale.connect_value_changed(move |scale| {
        let value = scale.value();
        {
            let mut state = state.borrow_mut();
            state.width = value;
            apply_selection(&mut state, |annotation| annotation.with_stroke_width(value));
        }
        ui_for_width.sync(&state);
    });
    panel.append(&width_scale);

    let text_label = gtk::Label::new(Some("Text size"));
    text_label.set_xalign(0.0);
    text_label.add_css_class("dim-label");
    panel.append(&text_label);
    let text_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 8.0, 96.0, 1.0);
    text_scale.set_value(22.0);
    text_scale.set_draw_value(true);
    let state = Rc::clone(inner);
    let ui_for_text = ui.clone();
    text_scale.connect_value_changed(move |scale| {
        let value = scale.value();
        {
            let mut state = state.borrow_mut();
            state.font_size = value;
            apply_selection(&mut state, |annotation| {
                if annotation.is_text() {
                    annotation.with_stroke_width(value)
                } else {
                    annotation.clone()
                }
            });
        }
        ui_for_text.sync(&state);
    });
    panel.append(&text_scale);

    let opacity_label = gtk::Label::new(Some("Highlighter opacity"));
    opacity_label.set_xalign(0.0);
    opacity_label.add_css_class("dim-label");
    panel.append(&opacity_label);
    let opacity_row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    let mut first: Option<gtk::ToggleButton> = None;
    for (index, (name, opacity)) in HIGHLIGHTER_PRESETS.iter().copied().enumerate() {
        let button = gtk::ToggleButton::with_label(name);
        if let Some(first) = &first {
            button.set_group(Some(first));
        } else {
            first = Some(button.clone());
        }
        button.set_active(index == 1);
        let state = Rc::clone(inner);
        let ui = ui.clone();
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            {
                let mut state = state.borrow_mut();
                state.highlighter_opacity = opacity;
                apply_selection(&mut state, |annotation| {
                    if matches!(
                        &annotation.kind,
                        AnnotationKind::Stroke(Stroke {
                            highlighter: true,
                            ..
                        })
                    ) {
                        annotation.with_highlighter_opacity(opacity)
                    } else {
                        annotation.clone()
                    }
                });
            }
            ui.sync(&state);
        });
        opacity_row.append(&button);
    }
    panel.append(&opacity_row);

    let export_label = gtk::Label::new(Some("Export format"));
    export_label.set_xalign(0.0);
    export_label.add_css_class("dim-label");
    panel.append(&export_label);
    let formats = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    let mut first_format: Option<gtk::ToggleButton> = None;
    let current_format = inner.borrow().export_format;
    for format in ExportFormat::ALL.iter().copied() {
        let button = gtk::ToggleButton::with_label(format.label());
        if let Some(first) = &first_format {
            button.set_group(Some(first));
        } else {
            first_format = Some(button.clone());
        }
        button.set_active(format == current_format);
        let state = Rc::clone(inner);
        button.connect_toggled(move |button| {
            if button.is_active() {
                state.borrow_mut().export_format = format;
            }
        });
        formats.append(&button);
    }
    panel.append(&formats);

    let hint = gtk::Label::new(Some(
        "Tip: scroll to zoom, middle-drag to pan. Double-click text with the Move tool to edit it.",
    ));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("caption");
    hint.add_css_class("dim-label");
    panel.append(&hint);

    panel.upcast()
}

#[allow(clippy::too_many_arguments)]
fn bottom_bar(
    inner: &Rc<RefCell<Inner>>,
    ui: &Ui,
    parent: &gtk::Window,
    undo: &gtk::Button,
    redo: &gtk::Button,
    crop_apply: &gtk::Button,
    crop_cancel: &gtk::Button,
    status: &gtk::Label,
) -> gtk::Widget {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.set_margin_start(8);
    bar.set_margin_end(8);
    bar.set_margin_bottom(8);
    bar.append(undo);
    bar.append(redo);

    let zoom_out = gtk::Button::with_label("−");
    zoom_out.set_tooltip_text(Some("Zoom out"));
    let zoom_in = gtk::Button::with_label("+");
    zoom_in.set_tooltip_text(Some("Zoom in"));
    let fit = gtk::Button::with_label("Fit");
    let actual = gtk::Button::with_label("100%");
    bar.append(&zoom_out);
    bar.append(&zoom_in);
    bar.append(&fit);
    bar.append(&actual);

    let reset_crop = gtk::Button::with_label("Reset Crop");
    reset_crop.set_tooltip_text(Some("Restore the full captured image"));
    bar.append(&reset_crop);
    bar.append(crop_apply);
    bar.append(crop_cancel);

    let copy = gtk::Button::with_label("Copy Annotated");
    copy.add_css_class("suggested-action");
    let extract = gtk::Button::with_label("Extract Text");
    extract.set_tooltip_text(Some("Recognise text in the current crop on this device"));
    let export = gtk::Button::with_label("Export…");
    bar.append(&copy);
    bar.append(&extract);
    bar.append(&export);
    let close = gtk::Button::with_label("Close Editor");
    bar.append(&close);
    bar.append(status);

    let state = Rc::clone(inner);
    let ui_for_undo = ui.clone();
    undo.connect_clicked(move |_| {
        if state.borrow_mut().editor.undo() {
            state.borrow_mut().selection = None;
            ui_for_undo.sync(&state);
        }
    });
    let state = Rc::clone(inner);
    let ui_for_redo = ui.clone();
    redo.connect_clicked(move |_| {
        if state.borrow_mut().editor.redo() {
            state.borrow_mut().selection = None;
            ui_for_redo.sync(&state);
        }
    });
    let state = Rc::clone(inner);
    let ui_for_zoom_out = ui.clone();
    zoom_out.connect_clicked(move |_| {
        zoom_by(&state, 1.0 / 1.25);
        ui_for_zoom_out.sync(&state);
    });
    let state = Rc::clone(inner);
    let ui_for_zoom_in = ui.clone();
    zoom_in.connect_clicked(move |_| {
        zoom_by(&state, 1.25);
        ui_for_zoom_in.sync(&state);
    });
    let state = Rc::clone(inner);
    let ui_for_fit = ui.clone();
    fit.connect_clicked(move |_| {
        fit_view(&state);
        ui_for_fit.sync(&state);
    });
    let state = Rc::clone(inner);
    let ui_for_actual = ui.clone();
    actual.connect_clicked(move |_| {
        zoom_to(&state, 1.0);
        ui_for_actual.sync(&state);
    });
    let state = Rc::clone(inner);
    let ui_for_reset = ui.clone();
    reset_crop.connect_clicked(move |_| {
        let span = {
            let state = state.borrow();
            state.editor.document().base_bounds()
        };
        let rect = span.to_pixel_rect(span);
        let result = {
            let mut state = state.borrow_mut();
            let Some(rect) = rect else {
                return;
            };
            state.editor.set_crop(rect)
        };
        match result {
            Ok(()) => {
                state.borrow_mut().selection = None;
                fit_view(&state);
                ui_for_reset.set_status("Restored the full captured image.");
            }
            Err(error) => ui_for_reset.toast(&format!("Could not reset the crop: {error}")),
        }
        ui_for_reset.sync(&state);
    });
    let state = Rc::clone(inner);
    let ui_for_apply = ui.clone();
    crop_apply.connect_clicked(move |_| {
        let candidate = state.borrow().crop_candidate;
        let Some(candidate) = candidate else {
            return;
        };
        let bounds = state.borrow().editor.document().base_bounds();
        let Some(rect) = candidate.to_pixel_rect(bounds) else {
            ui_for_apply.toast("The crop selection is too small or outside the image.");
            return;
        };
        let result = state.borrow_mut().editor.set_crop(rect);
        match result {
            Ok(()) => {
                let mut state = state.borrow_mut();
                state.crop_candidate = None;
                state.selection = None;
            }
            Err(error) => {
                ui_for_apply.toast(&format!("Could not apply the crop: {error}"));
                return;
            }
        }
        fit_view(&state);
        ui_for_apply.set_status("Crop applied. Undo restores the previous crop.");
        ui_for_apply.sync(&state);
    });
    let state = Rc::clone(inner);
    let ui_for_cancel = ui.clone();
    crop_cancel.connect_clicked(move |_| {
        state.borrow_mut().crop_candidate = None;
        ui_for_cancel.set_status("Crop cancelled.");
        ui_for_cancel.sync(&state);
    });
    let state = Rc::clone(inner);
    let ui_for_copy = ui.clone();
    copy.connect_clicked(move |_| {
        let _ = &state;
        match render_annotated(&state) {
            Ok(image) => {
                if let Some(display) = gdk::Display::default() {
                    let texture = texture_from_rendered(&image);
                    display.clipboard().set_texture(&texture);
                    (ui_for_copy.callbacks.copied)();
                    let message = format!(
                        "Copied annotated {} × {} to the clipboard.",
                        image.width(),
                        image.height()
                    );
                    ui_for_copy.set_status(&message);
                    ui_for_copy.toast(&message);
                } else {
                    ui_for_copy.toast("Could not access the display clipboard.");
                }
            }
            Err(error) => ui_for_copy.toast(&error),
        }
    });
    let state = Rc::clone(inner);
    let ui_for_extract = ui.clone();
    let parent_for_extract = parent.clone();
    extract.connect_clicked(move |_| start_ocr(&state, &ui_for_extract, &parent_for_extract));
    let state = Rc::clone(inner);
    let ui_for_export = ui.clone();
    let parent = parent.clone();
    export.connect_clicked(move |_| open_export_dialog_impl(&state, &ui_for_export, &parent));
    let callbacks = ui.callbacks.clone();
    close.connect_clicked(move |_| (callbacks.close)());

    bar.upcast()
}

fn zoom_by(inner: &Rc<RefCell<Inner>>, factor: f64) {
    let anchor = {
        let state = inner.borrow();
        state.cursor.unwrap_or_else(|| centre(&state))
    };
    let mut state = inner.borrow_mut();
    let zoom = state.view.zoom() * factor;
    state.view = state.view.zoomed_at(anchor, zoom);
}

fn zoom_to(inner: &Rc<RefCell<Inner>>, zoom: f64) {
    let anchor = {
        let state = inner.borrow();
        state.cursor.unwrap_or_else(|| centre(&state))
    };
    let mut state = inner.borrow_mut();
    state.view = state.view.zoomed_at(anchor, zoom);
}

fn fit_view(inner: &Rc<RefCell<Inner>>) {
    let mut state = inner.borrow_mut();
    let viewport = ScreenSize::new(
        state.view.viewport().width.max(1.0),
        state.view.viewport().height.max(1.0),
    );
    let crop = state.editor.document().crop_rect();
    state.view = ViewTransform::fit(crop, viewport);
    state.needs_fit = false;
}

fn centre(state: &Inner) -> ScreenPoint {
    ScreenPoint::new(
        state.view.viewport().width / 2.0,
        state.view.viewport().height / 2.0,
    )
}

fn apply_selection(state: &mut Inner, transform: impl Fn(&Annotation) -> Annotation) {
    let Some(id) = state.selection else {
        return;
    };
    let Some(annotation) = state.editor.document().annotation(id).cloned() else {
        return;
    };
    let after = transform(&annotation);
    let _ = state.editor.replace_annotation(after);
}

fn install_draw(canvas: &gtk::DrawingArea, inner: &Rc<RefCell<Inner>>) {
    let state = Rc::clone(inner);
    canvas.set_draw_func(move |_area, cr, width, height| {
        let mut state = state.borrow_mut();
        let viewport = ScreenSize::new(f64::from(width), f64::from(height));
        if viewport != state.view.viewport() {
            state.view = state.view.with_viewport(viewport);
            if state.needs_fit {
                let crop = state.editor.document().crop_rect();
                state.view = ViewTransform::fit(crop, viewport);
                state.needs_fit = false;
            }
        }

        cr.set_source_rgb(0.10, 0.10, 0.11);
        let _ = cr.paint();

        let Some(source) = state.view.visible_doc_rect() else {
            return;
        };
        let scale = state.view.zoom();
        let origin = state.view.screen_from_doc(source.origin());
        let rendered = {
            let Inner {
                editor,
                engine,
                preview,
                ..
            } = &mut *state;
            let document = editor.document();
            let owned = preview.as_ref().map(|preview| {
                if preview.replace {
                    Preview::Replace(&preview.annotation)
                } else {
                    Preview::Add(&preview.annotation)
                }
            });
            engine.render(document, owned.unwrap_or(Preview::None), source, scale)
        };
        if let Ok(image) = rendered {
            if let Some(surface) = surface_for(&image) {
                let _ = cr.set_source_surface(&surface, origin.x, origin.y);
                let _ = cr.paint();
            }
        }

        draw_overlays(&state, cr, viewport);
    });
}

fn draw_overlays(state: &Inner, cr: &cairo::Context, viewport: ScreenSize) {
    if let Some(candidate) = state.crop_candidate {
        let rect = state.view.screen_rect_from_doc(candidate);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        let top = gtk::cairo::Rectangle::new(0.0, 0.0, viewport.width, rect.y.max(0.0));
        fill_rect(cr, top);
        let bottom_y = (rect.bottom()).min(viewport.height);
        fill_rect(
            cr,
            gtk::cairo::Rectangle::new(
                0.0,
                bottom_y,
                viewport.width,
                (viewport.height - bottom_y).max(0.0),
            ),
        );
        let left = gtk::cairo::Rectangle::new(0.0, rect.y.max(0.0), rect.x.max(0.0), rect.height);
        fill_rect(cr, left);
        let right_x = rect.right().min(viewport.width);
        fill_rect(
            cr,
            gtk::cairo::Rectangle::new(
                right_x,
                rect.y.max(0.0),
                (viewport.width - right_x).max(0.0),
                rect.height,
            ),
        );
        cr.set_source_rgba(0.30, 0.70, 1.0, 1.0);
        cr.set_line_width(2.0);
        cr.rectangle(rect.x, rect.y, rect.width, rect.height);
        let _ = cr.stroke();
    }

    if let Some(id) = state.selection {
        if let Some(annotation) = state.editor.document().annotation(id) {
            let bounds = annotation.bounds();
            let rect = state.view.screen_rect_from_doc(bounds);
            cr.set_source_rgba(0.25, 0.65, 1.0, 0.95);
            cr.set_line_width(1.5);
            cr.rectangle(rect.x, rect.y, rect.width, rect.height);
            let _ = cr.stroke();
            for handle in Handle::ALL {
                let point = state.view.screen_from_doc(handle.position(bounds));
                cr.rectangle(
                    point.x - HANDLE_SIZE / 2.0,
                    point.y - HANDLE_SIZE / 2.0,
                    HANDLE_SIZE,
                    HANDLE_SIZE,
                );
                cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(0.25, 0.65, 1.0, 1.0);
                let _ = cr.stroke();
            }
        }
    }

    if let Some(ocr) = &state.ocr {
        for (index, line) in ocr.result.lines.iter().enumerate() {
            let document = DocRect::new(
                ocr.origin.x + f64::from(line.rect.x),
                ocr.origin.y + f64::from(line.rect.y),
                f64::from(line.rect.width.max(1)),
                f64::from(line.rect.height.max(1)),
            );
            let rect = state.view.screen_rect_from_doc(document);
            let selected = ocr.selected == Some(index);
            if selected {
                cr.set_source_rgba(0.25, 0.65, 1.0, 0.30);
            } else {
                cr.set_source_rgba(1.0, 0.85, 0.20, 0.18);
            }
            fill_rect(
                cr,
                gtk::cairo::Rectangle::new(rect.x, rect.y, rect.width, rect.height),
            );
            cr.set_source_rgba(1.0, 0.75, 0.10, if selected { 1.0 } else { 0.75 });
            cr.set_line_width(if selected { 2.0 } else { 1.0 });
            cr.rectangle(rect.x, rect.y, rect.width, rect.height);
            let _ = cr.stroke();
        }
    }

    if state.tool == Tool::Eraser {
        if let Some(cursor) = state.cursor {
            let radius = (state.width / 2.0 * state.view.zoom()).max(5.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
            cr.set_line_width(1.5);
            cr.arc(cursor.x, cursor.y, radius, 0.0, std::f64::consts::TAU);
            let _ = cr.stroke();
        }
    }
}

fn fill_rect(cr: &cairo::Context, rect: cairo::Rectangle) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    cr.rectangle(rect.x(), rect.y(), rect.width(), rect.height());
    let _ = cr.fill();
}

fn install_canvas_input(
    canvas: &gtk::DrawingArea,
    inner: &Rc<RefCell<Inner>>,
    ui: &Ui,
    parent: &gtk::Window,
) {
    let motion = gtk::EventControllerMotion::new();
    let state = Rc::clone(inner);
    let ui_for_motion = ui.clone();
    motion.connect_motion(move |_, x, y| {
        state.borrow_mut().cursor = Some(ScreenPoint::new(x, y));
        if state.borrow().tool == Tool::Eraser {
            ui_for_motion.canvas.queue_draw();
        }
    });
    let state = Rc::clone(inner);
    let ui_for_leave = ui.clone();
    motion.connect_leave(move |_| {
        state.borrow_mut().cursor = None;
        ui_for_leave.canvas.queue_draw();
    });
    canvas.add_controller(motion);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    let state = Rc::clone(inner);
    let ui_for_scroll = ui.clone();
    scroll.connect_scroll(move |_, _dx, dy| {
        let anchor = {
            let state = state.borrow();
            state.cursor.unwrap_or_else(|| centre(&state))
        };
        let factor = if dy < 0.0 { 1.15 } else { 1.0 / 1.15 };
        {
            let mut state = state.borrow_mut();
            let zoom = state.view.zoom() * factor;
            state.view = state.view.zoomed_at(anchor, zoom);
        }
        ui_for_scroll.sync(&state);
        glib::Propagation::Stop
    });
    canvas.add_controller(scroll);

    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let state = Rc::clone(inner);
    let ui_for_begin = ui.clone();
    drag.connect_drag_begin(move |_, x, y| {
        begin_drag(&state, &ui_for_begin, ScreenPoint::new(x, y));
    });
    let state = Rc::clone(inner);
    let ui_for_update = ui.clone();
    drag.connect_drag_update(move |_, offset_x, offset_y| {
        update_drag(&state, &ui_for_update, offset_x, offset_y);
    });
    let state = Rc::clone(inner);
    let ui_for_end = ui.clone();
    drag.connect_drag_end(move |_, _offset_x, _offset_y| {
        end_drag(&state, &ui_for_end);
    });
    canvas.add_controller(drag);

    let pan = gtk::GestureDrag::new();
    pan.set_button(gdk::BUTTON_MIDDLE);
    let state = Rc::clone(inner);
    pan.connect_drag_begin(move |_, _x, _y| {
        let mut state = state.borrow_mut();
        let pan = state.view.pan();
        state.interaction = Some(Interaction::Pan { pan });
    });
    let state = Rc::clone(inner);
    let ui_for_pan = ui.clone();
    pan.connect_drag_update(move |_, offset_x, offset_y| {
        {
            let mut state = state.borrow_mut();
            let Some(Interaction::Pan { pan }) = state.interaction else {
                return;
            };
            let target = ScreenPoint::new(pan.x + offset_x, pan.y + offset_y);
            let current = state.view.pan();
            state.view = state
                .view
                .panned_by(target.x - current.x, target.y - current.y);
        }
        ui_for_pan.canvas.queue_draw();
    });
    let state = Rc::clone(inner);
    pan.connect_drag_end(move |_, _offset_x, _offset_y| {
        state.borrow_mut().interaction = None;
    });
    canvas.add_controller(pan);

    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let state = Rc::clone(inner);
    let ui_for_click = ui.clone();
    let parent = parent.clone();
    click.connect_pressed(move |_, presses, x, y| {
        let point = ScreenPoint::new(x, y);
        let tool = state.borrow().tool;
        if tool == Tool::Eraser {
            state.borrow_mut().drag_moved = false;
        }
        if tool == Tool::Move {
            let hit = {
                let state = state.borrow();
                let document = state.view.doc_from_screen(point);
                state.ocr.as_ref().and_then(|overlay| {
                    let x = (document.x - overlay.origin.x).max(0.0) as u32;
                    let y = (document.y - overlay.origin.y).max(0.0) as u32;
                    overlay
                        .result
                        .line_at(x, y)
                        .and_then(|line| overlay.result.lines.iter().position(|item| item == line))
                })
            };
            if let Some(index) = hit {
                if let Some(overlay) = state.borrow_mut().ocr.as_mut() {
                    select_ocr_line(overlay, index);
                }
                ui_for_click.canvas.queue_draw();
                return;
            }
        }
        match tool {
            Tool::Text if presses == 1 => {
                let document = state.borrow().view.doc_from_screen(point);
                open_text_dialog(&state, &ui_for_click, &parent, None, document);
            }
            Tool::Move if presses >= 2 => {
                let document = state.borrow().view.doc_from_screen(point);
                let hit = state
                    .borrow()
                    .editor
                    .document()
                    .find_topmost(document, tolerance(&state))
                    .filter(|annotation| annotation.is_text())
                    .map(|annotation| annotation.id);
                if let Some(id) = hit {
                    open_text_dialog(&state, &ui_for_click, &parent, Some(id), document);
                }
            }
            Tool::Move => {
                let document = state.borrow().view.doc_from_screen(point);
                let hit = state
                    .borrow()
                    .editor
                    .document()
                    .find_topmost(document, tolerance(&state))
                    .map(|annotation| annotation.id);
                state.borrow_mut().selection = hit;
                ui_for_click.sync(&state);
            }
            _ => {}
        }
    });
    let state = Rc::clone(inner);
    let ui_for_release = ui.clone();
    click.connect_released(move |_, presses, x, y| {
        if presses != 1 {
            return;
        }
        let removed = {
            let mut state = state.borrow_mut();
            if state.tool != Tool::Eraser || state.drag_moved {
                return;
            }
            let document = state.view.doc_from_screen(ScreenPoint::new(x, y));
            let tolerance = state.width / 2.0 / state.view.zoom();
            let hit = state
                .editor
                .document()
                .find_topmost(document, tolerance)
                .map(|annotation| annotation.id);
            if let Some(id) = hit {
                state.editor.remove_annotations(&[id]);
                state.selection = None;
                true
            } else {
                false
            }
        };
        if removed {
            ui_for_release.sync(&state);
        }
    });
    canvas.add_controller(click);
}

fn tolerance(state: &Rc<RefCell<Inner>>) -> f64 {
    4.0 / state.borrow().view.zoom()
}

fn begin_drag(inner: &Rc<RefCell<Inner>>, ui: &Ui, start: ScreenPoint) {
    let document = {
        let state = inner.borrow();
        state.view.doc_from_screen(start)
    };
    let mut state = inner.borrow_mut();
    state.drag_start = Some(start);
    state.drag_moved = false;
    let tool = state.tool;
    match tool {
        Tool::Move => {
            if let Some(id) = state.selection {
                if let Some(annotation) = state.editor.document().annotation(id).cloned() {
                    let bounds = annotation.bounds();
                    for handle in Handle::ALL {
                        let handle_point = state.view.screen_from_doc(handle.position(bounds));
                        if (handle_point.x - start.x).hypot(handle_point.y - start.y)
                            <= HANDLE_PICK_RADIUS
                        {
                            state.preview = Some(OwnedPreview {
                                annotation: annotation.clone(),
                                replace: true,
                            });
                            state.interaction = Some(Interaction::Resize {
                                original: annotation,
                                handle,
                            });
                            return;
                        }
                    }
                }
            }
            let hit = state
                .editor
                .document()
                .find_topmost(document, 4.0 / state.view.zoom())
                .cloned();
            if let Some(annotation) = hit {
                state.selection = Some(annotation.id);
                state.preview = Some(OwnedPreview {
                    annotation: annotation.clone(),
                    replace: true,
                });
                state.interaction = Some(Interaction::Move {
                    original: annotation,
                    start: document,
                });
            } else {
                state.selection = None;
            }
        }
        Tool::Pen | Tool::Highlighter => {
            let highlighter = tool == Tool::Highlighter;
            let width = if highlighter {
                state.width * 2.0
            } else {
                state.width
            };
            let kind = AnnotationKind::Stroke(Stroke {
                points: vec![document],
                color: state.color,
                width,
                highlighter,
                opacity: state.highlighter_opacity,
            });
            let annotation = state.editor.create_annotation(kind);
            state.preview = Some(OwnedPreview {
                annotation,
                replace: false,
            });
            state.interaction = Some(Interaction::Stroke { last: document });
        }
        Tool::Line => {
            let kind = AnnotationKind::Line(Line {
                start: document,
                end: document,
                color: state.color,
                width: state.width,
            });
            state.preview = Some(OwnedPreview {
                annotation: state.editor.create_annotation(kind),
                replace: false,
            });
            state.interaction = Some(Interaction::Shape { start: document });
        }
        Tool::Arrow => {
            let kind = AnnotationKind::Arrow(Arrow {
                start: document,
                end: document,
                color: state.color,
                width: state.width,
            });
            state.preview = Some(OwnedPreview {
                annotation: state.editor.create_annotation(kind),
                replace: false,
            });
            state.interaction = Some(Interaction::Shape { start: document });
        }
        Tool::Rectangle | Tool::Ellipse => {
            let rect = DocRect::new(document.x, document.y, 0.0, 0.0);
            let shape = Shape {
                rect,
                color: state.color,
                width: state.width,
            };
            let kind = if tool == Tool::Rectangle {
                AnnotationKind::Rectangle(shape)
            } else {
                AnnotationKind::Ellipse(shape)
            };
            state.preview = Some(OwnedPreview {
                annotation: state.editor.create_annotation(kind),
                replace: false,
            });
            state.interaction = Some(Interaction::Shape { start: document });
        }
        Tool::Eraser => {
            state.interaction = Some(Interaction::Erase { ids: Vec::new() });
            erase_at(&mut state, document);
        }
        Tool::Crop => {
            state.crop_candidate = Some(DocRect::new(document.x, document.y, 0.0, 0.0));
            state.interaction = Some(Interaction::Crop { start: document });
        }
        Tool::Text => {}
    }
    ui.canvas.queue_draw();
}

fn update_drag(inner: &Rc<RefCell<Inner>>, ui: &Ui, offset_x: f64, offset_y: f64) {
    let mut state = inner.borrow_mut();
    let Some(start_screen) = state.drag_start else {
        return;
    };
    state.drag_moved = true;
    if matches!(state.interaction, Some(Interaction::Pan { .. }) | None) {
        return;
    }
    let current = ScreenPoint::new(start_screen.x + offset_x, start_screen.y + offset_y);
    let document = state.view.doc_from_screen(current);
    let zoom = state.view.zoom();
    let eraser_tolerance = state.width / 2.0 / zoom;
    let erase_hit = state
        .editor
        .document()
        .find_topmost(document, eraser_tolerance)
        .map(|annotation| annotation.id);
    match state.interaction.as_mut() {
        Some(Interaction::Stroke { last }) => {
            let minimum = 0.5 / zoom;
            if document.distance(*last) >= minimum {
                *last = document;
                if let Some(preview) = state.preview.as_mut() {
                    if let AnnotationKind::Stroke(stroke) = &mut preview.annotation.kind {
                        stroke.points.push(document);
                    }
                }
            }
        }
        Some(Interaction::Shape { start }) => {
            let start = *start;
            if let Some(preview) = state.preview.as_mut() {
                match &mut preview.annotation.kind {
                    AnnotationKind::Line(line) => line.end = document,
                    AnnotationKind::Arrow(arrow) => arrow.end = document,
                    AnnotationKind::Rectangle(shape) | AnnotationKind::Ellipse(shape) => {
                        shape.rect = DocRect::from_corners(start, document);
                    }
                    _ => {}
                }
            }
        }
        Some(Interaction::Move {
            original, start, ..
        }) => {
            let mut moved = original.clone();
            moved.translate(document.x - start.x, document.y - start.y);
            if let Some(preview) = state.preview.as_mut() {
                preview.annotation = moved;
            }
        }
        Some(Interaction::Resize {
            original, handle, ..
        }) => {
            let handle = *handle;
            let original = original.clone();
            let from = original.bounds();
            let mut to = from;
            if handle.moves_left() {
                to.x = document.x.min(from.right() - MIN_SHAPE_SIZE);
                to.width = from.right() - to.x;
            }
            if handle.moves_right() {
                let right = document.x.max(from.x + MIN_SHAPE_SIZE);
                to.width = right - from.x;
            }
            if handle.moves_top() {
                to.y = document.y.min(from.bottom() - MIN_SHAPE_SIZE);
                to.height = from.bottom() - to.y;
            }
            if handle.moves_bottom() {
                let bottom = document.y.max(from.y + MIN_SHAPE_SIZE);
                to.height = bottom - from.y;
            }
            let mut resized = original;
            resized.resize_to(to);
            if let Some(preview) = state.preview.as_mut() {
                preview.annotation = resized;
            }
        }
        Some(Interaction::Erase { ids }) => {
            if let Some(annotation) = erase_hit {
                if !ids.contains(&annotation) {
                    ids.push(annotation);
                }
            }
        }
        Some(Interaction::Crop { start }) => {
            let start = *start;
            let bounds = state.editor.document().crop_rect();
            let rect = DocRect::from_corners(start, document)
                .intersection(bounds)
                .unwrap_or(DocRect::new(start.x, start.y, 0.0, 0.0));
            state.crop_candidate = Some(rect);
        }
        Some(Interaction::Pan { .. }) | None => {}
    }
    drop(state);
    ui.canvas.queue_draw();
}

fn end_drag(inner: &Rc<RefCell<Inner>>, ui: &Ui) {
    let mut state = inner.borrow_mut();
    let interaction = state.interaction.take();
    match interaction {
        Some(Interaction::Stroke { .. }) => {
            if let Some(preview) = state.preview.take() {
                let id = state.editor.add_annotation(preview.annotation);
                state.selection = Some(id);
            }
        }
        Some(Interaction::Shape { .. }) => {
            if let Some(preview) = state.preview.take() {
                let bounds = preview.annotation.bounds();
                if bounds.width >= MIN_SHAPE_SIZE || bounds.height >= MIN_SHAPE_SIZE {
                    let id = state.editor.add_annotation(preview.annotation);
                    state.selection = Some(id);
                }
            }
        }
        Some(Interaction::Move { .. }) | Some(Interaction::Resize { .. }) => {
            if let Some(preview) = state.preview.take() {
                let _ = state.editor.replace_annotation(preview.annotation);
            }
        }
        Some(Interaction::Erase { ids }) => {
            if !ids.is_empty() {
                state.editor.remove_annotations(&ids);
            }
        }
        Some(Interaction::Crop { .. }) => {
            if let Some(candidate) = state.crop_candidate {
                if candidate.width < MIN_CROP_SIZE || candidate.height < MIN_CROP_SIZE {
                    state.crop_candidate = None;
                }
            }
        }
        Some(Interaction::Pan { .. }) | None => {}
    }
    state.preview = None;
    state.drag_start = None;
    drop(state);
    ui.sync(inner);
}

fn erase_at(state: &mut Inner, point: DocPoint) {
    let tolerance = state.width / 2.0 / state.view.zoom();
    if let Some(annotation) = state
        .editor
        .document()
        .find_topmost(point, tolerance)
        .map(|annotation| annotation.id)
    {
        if let Some(Interaction::Erase { ids }) = state.interaction.as_mut() {
            if !ids.contains(&annotation) {
                ids.push(annotation);
            }
        }
    }
}

fn open_text_dialog(
    inner: &Rc<RefCell<Inner>>,
    ui: &Ui,
    parent: &gtk::Window,
    existing: Option<AnnotationId>,
    at: DocPoint,
) {
    let dialog = adw::Dialog::builder()
        .title("Text annotation")
        .content_width(440)
        .content_height(300)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(140)
        .build();
    content.append(&scroller);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save_button = gtk::Button::with_label("Save text");
    save_button.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&save_button);
    content.append(&actions);
    dialog.set_child(Some(&content));

    if let Some(id) = existing {
        if let Some(annotation) = inner.borrow().editor.document().annotation(id) {
            if let AnnotationKind::Text(text) = &annotation.kind {
                view.buffer().set_text(&text.text);
            }
        }
    }

    let weak_dialog = dialog.downgrade();
    let cancel_dialog = weak_dialog.clone();
    cancel.connect_clicked(move |_| {
        if let Some(dialog) = cancel_dialog.upgrade() {
            dialog.close();
        }
    });

    let state = Rc::clone(inner);
    let ui = ui.clone();
    let save_dialog = weak_dialog;
    save_button.connect_clicked(move |_| {
        let buffer = view.buffer();
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .to_string();
        {
            let mut state = state.borrow_mut();
            if let Some(id) = existing {
                let measured = state
                    .editor
                    .document()
                    .annotation(id)
                    .and_then(|annotation| match &annotation.kind {
                        AnnotationKind::Text(text) => Some((text.font_size, text.width)),
                        _ => None,
                    });
                let (font_size, width) = measured.unwrap_or((state.font_size, DEFAULT_TEXT_WIDTH));
                let height = state
                    .engine
                    .measure_text(&text, font_size, width)
                    .1
                    .max(font_size);
                state.editor.edit_text(id, text, height);
                state.selection = Some(id);
            } else if !text.is_empty() {
                let font_size = state.font_size;
                let width = DEFAULT_TEXT_WIDTH;
                let height = state
                    .engine
                    .measure_text(&text, font_size, width)
                    .1
                    .max(font_size);
                let kind = AnnotationKind::Text(Text {
                    origin: at,
                    width,
                    height,
                    text,
                    color: state.color,
                    font_size,
                });
                let annotation = state.editor.create_annotation(kind);
                let id = state.editor.add_annotation(annotation);
                state.selection = Some(id);
            }
        }
        ui.sync(&state);
        if let Some(dialog) = save_dialog.upgrade() {
            dialog.close();
        }
    });

    dialog.present(Some(parent));
}

fn persist_editor_settings(inner: &Inner) {
    let settings = inner.settings.borrow().clone();
    if let Err(error) = settings.save(&inner.settings_path) {
        tracing::warn!(%error, "could not save OCR settings");
    }
}

fn start_ocr(inner: &Rc<RefCell<Inner>>, ui: &Ui, parent: &gtk::Window) {
    if inner.borrow().ocr_busy.replace(true) {
        ui.toast("Text recognition is already running.");
        return;
    }
    let previous = inner.borrow_mut().ocr.take();
    if let Some(overlay) = previous {
        overlay.dialog.window.close();
    }
    let image = match render_annotated(inner) {
        Ok(image) => image,
        Err(error) => {
            inner.borrow().ocr_busy.set(false);
            ui.toast(&error);
            return;
        }
    };
    let crop = inner.borrow().editor.document().crop_rect();
    let origin = DocPoint::new(crop.x, crop.y);
    let settings = Rc::clone(&inner.borrow().settings);
    let configured = settings.borrow().model_directory();
    let models = snipunix_ocr::ModelPaths::discover(Some(&configured))
        .or_else(|_| snipunix_ocr::ModelPaths::discover(None));
    let models = match models {
        Ok(models) => models,
        Err(error) => {
            inner.borrow().ocr_busy.set(false);
            prompt_download_models(inner, ui, parent, &error.to_string());
            return;
        }
    };
    let remove_line_breaks = settings.borrow().ocr.remove_line_breaks;
    let auto_copy = settings.borrow().ocr.auto_copy;

    ui.set_status("Recognising text locally…");
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = snipunix_ocr::recognize(image.rgba(), image.width(), image.height(), &models)
            .map_err(|error| error.to_string());
        let _ = sender.send_blocking(result);
    });

    let inner = Rc::clone(inner);
    let ui = ui.clone();
    let parent = parent.clone();
    glib::spawn_future_local(async move {
        let result = match receiver.recv().await {
            Ok(result) => result,
            Err(_) => Err("the OCR worker stopped unexpectedly".to_string()),
        };
        inner.borrow().ocr_busy.set(false);
        match result {
            Ok(result) if result.is_empty() => {
                ui.set_status("No text was recognised in the current crop.");
                ui.toast("No text was recognised in the current crop.");
            }
            Ok(result) => {
                if auto_copy {
                    let text = result.text(remove_line_breaks);
                    if !text.is_empty() {
                        copy_text_to_clipboard(&text);
                    }
                }
                ui.set_status(&format!(
                    "Recognised {} text line(s) locally.",
                    result.lines.len()
                ));
                show_ocr_dialog(&inner, &ui, &parent, result, origin);
            }
            Err(error) => {
                ui.set_status("Text recognition failed.");
                ui.toast(&format!("Text recognition failed: {error}"));
            }
        }
    });
}

fn prompt_download_models(inner: &Rc<RefCell<Inner>>, ui: &Ui, parent: &gtk::Window, detail: &str) {
    let dialog = adw::MessageDialog::builder()
        .heading("Text recognition needs a local model")
        .body(format!(
            "OCR runs on this device. Snipunix needs two model files (about 12 MB).\n\n{detail}"
        ))
        .transient_for(parent)
        .modal(true)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("download", "Download Models");
    dialog.set_response_appearance("download", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let inner = Rc::clone(inner);
    let ui = ui.clone();
    let parent = parent.clone();
    glib::spawn_future_local(async move {
        if dialog.choose_future().await != "download" {
            ui.set_status("Text recognition cancelled: the OCR models are not installed.");
            return;
        }
        let directory = inner.borrow().settings.borrow().model_directory();
        ui.set_status("Downloading and verifying OCR models…");
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = snipunix_ocr::download_models(&directory)
                .map(|_| ())
                .map_err(|error| error.to_string());
            let _ = sender.send_blocking(result);
        });
        match receiver.recv().await {
            Ok(Ok(())) => {
                ui.toast("OCR models installed.");
                start_ocr(&inner, &ui, &parent);
            }
            Ok(Err(error)) => {
                ui.set_status("Could not install OCR models.");
                ui.toast(&format!("Could not install OCR models: {error}"));
            }
            Err(_) => ui.toast("The OCR model download stopped unexpectedly."),
        }
    });
}

fn show_ocr_dialog(
    inner: &Rc<RefCell<Inner>>,
    ui: &Ui,
    parent: &gtk::Window,
    result: OcrResult,
    origin: DocPoint,
) {
    let window = adw::Window::builder()
        .title("Extracted text")
        .transient_for(parent)
        .modal(false)
        .default_width(620)
        .default_height(520)
        .build();
    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let buffer = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
    let text_view = gtk::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .cursor_visible(true)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&text_view)
        .build();

    let remove_line_breaks = gtk::Switch::new();
    remove_line_breaks.set_active(inner.borrow().settings.borrow().ocr.remove_line_breaks);
    remove_line_breaks.set_valign(gtk::Align::Center);
    let auto_copy = gtk::Switch::new();
    auto_copy.set_active(inner.borrow().settings.borrow().ocr.auto_copy);
    auto_copy.set_valign(gtk::Align::Center);

    let options = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    options.set_margin_top(8);
    options.set_margin_start(12);
    options.set_margin_end(12);
    let remove_label = gtk::Label::new(Some("Remove line breaks"));
    remove_label.set_xalign(0.0);
    remove_label.set_hexpand(true);
    options.append(&remove_label);
    options.append(&remove_line_breaks);
    let auto_label = gtk::Label::new(Some("Auto-copy"));
    auto_label.set_xalign(0.0);
    options.append(&auto_label);
    options.append(&auto_copy);

    let detected = result.redaction_rects().len();
    let redact_label = if detected == 0 {
        "No sensitive text".to_string()
    } else {
        format!("Redact {detected} detected")
    };
    let redact = gtk::Button::with_label(&redact_label);
    redact.set_sensitive(detected > 0);
    redact.set_tooltip_text(Some(
        "Cover detected emails and phone numbers with opaque black",
    ));

    let copy = gtk::Button::with_label("Copy All");
    copy.add_css_class("suggested-action");
    let close = gtk::Button::with_label("Close");
    let status = gtk::Label::new(Some(&format!(
        "{} line(s) recognised locally.",
        result.lines.len()
    )));
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.add_css_class("dim-label");

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_margin_top(8);
    actions.set_margin_bottom(8);
    actions.set_margin_start(12);
    actions.set_margin_end(12);
    actions.append(&status);
    actions.append(&redact);
    actions.append(&copy);
    actions.append(&close);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&scrolled);
    content.append(&options);
    content.append(&actions);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    let remove_line_breaks_value = inner.borrow().settings.borrow().ocr.remove_line_breaks;
    buffer.set_text(&result.text(remove_line_breaks_value));

    let overlay = OcrOverlay {
        result,
        origin,
        selected: None,
        dialog: OcrDialog {
            window: window.clone(),
            buffer: buffer.clone(),
            text_view: text_view.clone(),
            remove_line_breaks: remove_line_breaks.clone(),
            redact: redact.clone(),
            status: status.clone(),
        },
    };
    inner.borrow_mut().ocr = Some(overlay);
    ui.sync(inner);

    {
        let inner = Rc::clone(inner);
        remove_line_breaks.connect_active_notify(move |switch| {
            let remove = switch.is_active();
            let text = {
                let state = inner.borrow();
                state.settings.borrow_mut().ocr.remove_line_breaks = remove;
                persist_editor_settings(&state);
                let Some(overlay) = state.ocr.as_ref() else {
                    return;
                };
                overlay.result.text(remove)
            };
            if let Some(overlay) = inner.borrow().ocr.as_ref() {
                overlay.dialog.buffer.set_text(&text);
                overlay.dialog.remove_line_breaks.set_active(remove);
            }
        });
    }
    {
        let inner = Rc::clone(inner);
        auto_copy.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            let state = inner.borrow();
            state.settings.borrow_mut().ocr.auto_copy = enabled;
            persist_editor_settings(&state);
        });
    }
    {
        let inner = Rc::clone(inner);
        let ui = ui.clone();
        copy.connect_clicked(move |_| {
            let text = inner
                .borrow()
                .ocr
                .as_ref()
                .map(|overlay| {
                    overlay
                        .dialog
                        .buffer
                        .text(
                            &overlay.dialog.buffer.start_iter(),
                            &overlay.dialog.buffer.end_iter(),
                            false,
                        )
                        .to_string()
                })
                .unwrap_or_default();
            if text.is_empty() {
                ui.toast("There is no recognised text to copy.");
                return;
            }
            if copy_text_to_clipboard(&text) {
                ui.set_status("Copied recognised text to the clipboard.");
                ui.toast("Copied recognised text to the clipboard.");
            } else {
                ui.toast("Could not access the display clipboard.");
            }
        });
    }
    {
        let inner = Rc::clone(inner);
        let ui = ui.clone();
        redact.connect_clicked(move |_| {
            let applied = apply_ocr_redactions(&inner);
            match applied {
                Ok(0) => ui.toast("No emails or phone numbers were detected."),
                Ok(count) => {
                    let message = format!(
                        "Redacted {count} detected region(s) with opaque black. The black fill is part of the image output."
                    );
                    ui.set_status(&message);
                    ui.toast(&message);
                }
                Err(error) => ui.toast(&error),
            }
            ui.sync(&inner);
        });
    }
    {
        let window_for_close = window.clone();
        close.connect_clicked(move |_| window_for_close.close());
    }
    {
        let inner = Rc::clone(inner);
        let ui = ui.clone();
        window.connect_close_request(move |_| {
            inner.borrow_mut().ocr = None;
            ui.sync(&inner);
            glib::Propagation::Proceed
        });
    }
    window.present();
}

fn copy_text_to_clipboard(text: &str) -> bool {
    let Some(display) = gdk::Display::default() else {
        return false;
    };
    display.clipboard().set_text(text);
    true
}

fn select_ocr_line(overlay: &mut OcrOverlay, index: usize) {
    overlay.selected = Some(index);
    let buffer = &overlay.dialog.buffer;
    let Some(mut start) = buffer.iter_at_line(i32::try_from(index).unwrap_or(0)) else {
        return;
    };
    let end = buffer
        .iter_at_line(i32::try_from(index + 1).unwrap_or(i32::MAX))
        .unwrap_or_else(|| buffer.end_iter());
    buffer.select_range(&start, &end);
    let _ = overlay
        .dialog
        .text_view
        .scroll_to_iter(&mut start, 0.0, false, 0.0, 0.0);
}

fn apply_ocr_redactions(inner: &Rc<RefCell<Inner>>) -> Result<usize, String> {
    let (rectangles, origin) = {
        let state = inner.borrow();
        let Some(overlay) = state.ocr.as_ref() else {
            return Ok(0);
        };
        (overlay.result.redaction_rects(), overlay.origin)
    };
    if rectangles.is_empty() {
        return Ok(0);
    }
    let mut state = inner.borrow_mut();
    let mut annotations = Vec::with_capacity(rectangles.len());
    for rectangle in &rectangles {
        let rect = DocRect::new(
            origin.x + f64::from(rectangle.x),
            origin.y + f64::from(rectangle.y),
            f64::from(rectangle.width.max(1)),
            f64::from(rectangle.height.max(1)),
        );
        let annotation = state
            .editor
            .create_annotation(AnnotationKind::Redaction(Redaction { rect }));
        annotations.push(annotation);
    }
    let count = annotations.len();
    state.editor.add_annotations(annotations);
    state.selection = None;
    if let Some(overlay) = state.ocr.as_mut() {
        overlay.selected = None;
        overlay.dialog.redact.set_sensitive(false);
        overlay.dialog.redact.set_label("Redacted");
        overlay
            .dialog
            .status
            .set_label("Redaction applied. The black pixels are part of the output.");
    }
    Ok(count)
}

fn render_annotated(inner: &Rc<RefCell<Inner>>) -> Result<RenderedImage, String> {
    let mut state = inner.borrow_mut();
    let crop = state.editor.document().crop_rect();
    let Inner { editor, engine, .. } = &mut *state;
    engine
        .render(editor.document(), Preview::None, crop, 1.0)
        .map_err(|error| format!("Could not render the annotated image: {error}"))
}

fn export_to(
    inner: &Rc<RefCell<Inner>>,
    ui: &Ui,
    parent: &gtk::Window,
    path: &std::path::Path,
    format: ExportFormat,
) -> Result<(), String> {
    if path.exists() {
        let dialog = adw::MessageDialog::builder()
            .heading("Replace existing file?")
            .body(format!(
                "{} already exists. Replacing it removes the old file.",
                path.display()
            ))
            .transient_for(parent)
            .modal(true)
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("replace", "Replace");
        dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let inner = Rc::clone(inner);
        let ui = ui.clone();
        let path = path.to_path_buf();
        glib::spawn_future_local(async move {
            let response = dialog.choose_future().await;
            if response != "replace" {
                ui.set_status("Export cancelled.");
                return;
            }
            write_export(&inner, &ui, &path, format);
        });
        return Ok(());
    }
    write_export(inner, ui, path, format);
    Ok(())
}

fn write_export(inner: &Rc<RefCell<Inner>>, ui: &Ui, path: &std::path::Path, format: ExportFormat) {
    match render_annotated(inner) {
        Ok(image) => match save(path, &image, format, 90) {
            Ok(()) => {
                let message = format!("Exported {} to {}", format.label(), path.display());
                ui.set_status(&message);
                ui.toast(&message);
            }
            Err(error) => ui.toast(&format!("Could not export to {}: {error}", path.display())),
        },
        Err(error) => ui.toast(&error),
    }
}

fn open_export_dialog_impl(inner: &Rc<RefCell<Inner>>, ui: &Ui, parent: &gtk::Window) {
    let format = inner.borrow().export_format;
    let dialog = gtk::FileDialog::builder()
        .title(format!("Export {} image", format.label()))
        .initial_name(default_export_name(format))
        .build();
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&format!(
        "{} image (*.{})",
        format.label(),
        format.extension()
    )));
    filter.add_mime_type(format.mime());
    filter.add_pattern(&format!("*.{}", format.extension()));
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));

    let state = Rc::clone(inner);
    let ui = ui.clone();
    let parent = parent.clone();
    let dialog_parent = parent.clone();
    dialog.save(
        Some(&dialog_parent),
        None::<&gio::Cancellable>,
        move |result| {
            let file = match result {
                Ok(file) => file,
                Err(error) => {
                    if !error.matches(gtk::DialogError::Dismissed) {
                        ui.toast(&format!("Could not choose an export location: {error}"));
                    }
                    return;
                }
            };
            let Some(path) = file.path() else {
                ui.toast("The selected export location is not a local file.");
                return;
            };
            let path = with_format_extension(path, format);
            if let Err(error) = export_to(&state, &ui, &parent, &path, format) {
                ui.toast(&error);
            }
        },
    );
}

fn with_format_extension(path: std::path::PathBuf, format: ExportFormat) -> std::path::PathBuf {
    let mut path = path;
    if path.extension().and_then(|extension| extension.to_str()) != Some(format.extension()) {
        path.set_extension(format.extension());
    }
    path
}

fn default_export_name(format: ExportFormat) -> String {
    let stamp = glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format("%Y%m%d-%H%M%S").ok())
        .map_or_else(|| "annotated".to_string(), |value| value.to_string());
    format!("snipunix-annotated-{stamp}.{}", format.extension())
}

fn texture_from_rendered(image: &RenderedImage) -> gdk::Texture {
    let bytes = glib::Bytes::from(image.rgba());
    let texture = gdk::MemoryTexture::new(
        image.width() as i32,
        image.height() as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        image.width() as usize * 4,
    );
    texture.upcast()
}

fn surface_for(image: &RenderedImage) -> Option<cairo::ImageSurface> {
    let width = i32::try_from(image.width()).ok()?;
    let height = i32::try_from(image.height()).ok()?;
    let stride = cairo::Format::ARgb32
        .stride_for_width(u32::try_from(width).ok()?)
        .ok()?;
    let mut data = vec![
        0_u8;
        usize::try_from(stride)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
    ];
    let source = image.rgba();
    for row in 0..usize::try_from(height).ok()? {
        let source_row = row * usize::try_from(image.width()).ok()? * 4;
        let target_row = row * usize::try_from(stride).ok()?;
        for column in 0..usize::try_from(width).ok()? {
            let source_index = source_row + column * 4;
            let target_index = target_row + column * 4;
            let (r, g, b, a) = (
                u32::from(source[source_index]),
                u32::from(source[source_index + 1]),
                u32::from(source[source_index + 2]),
                u32::from(source[source_index + 3]),
            );
            let premultiply = |value: u32| ((value * a + 127) / 255) as u8;
            #[cfg(target_endian = "little")]
            {
                data[target_index] = premultiply(b);
                data[target_index + 1] = premultiply(g);
                data[target_index + 2] = premultiply(r);
                data[target_index + 3] = a as u8;
            }
            #[cfg(target_endian = "big")]
            {
                data[target_index] = a as u8;
                data[target_index + 1] = premultiply(r);
                data[target_index + 2] = premultiply(g);
                data[target_index + 3] = premultiply(b);
            }
        }
    }
    cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, width, height, stride).ok()
}
