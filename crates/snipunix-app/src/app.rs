//! GTK4 + Libadwaita application shell and capture flow.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use snipunix_annotate::{ExportFormat, RenderedImage};
use snipunix_capture::{
    AutoCapture, CaptureBackend, CaptureError, CaptureFrame, CaptureOptions, CapturedImage,
    PixelMonitor,
};
use snipunix_core::{CaptureMapping, ImageRect, LogicalRect, MappingError, MonitorImage};
use snipunix_overlay::{
    OverlayFrames, OverlayOptions, OverlayResult, collect_monitors, layout_from_monitors, present,
    start_countdown,
};

use snipunix_settings::{CaptureMode, Settings};

use crate::cli::{CaptureTarget, Cli};
use crate::editor::{self, EditorView};
use crate::onboarding;
use crate::settings_ui;
use crate::shortcuts::{self, BoundShortcut, ShortcutEvent};
use crate::tray::{self, TrayCommand};

pub const APP_ID: &str = "io.github.alokverma9.snipunix";

pub fn run(cli: Cli) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let initial = Rc::new(RefCell::new(Some(cli)));

    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            // A second launch (or a hidden resident window) focuses the window
            // that already exists instead of building a duplicate.
            window.present();
            return;
        }
        let cli = initial.borrow_mut().take().unwrap_or_default();
        activate(app, cli);
    });

    app.set_accels_for_action("app.capture-region", &["<Control><Shift>s"]);
    app.set_accels_for_action("app.capture-screen", &["<Control><Shift>a"]);
    app.set_accels_for_action("app.capture-window", &["<Control><Shift>w"]);
    app.set_accels_for_action("app.annotate", &["<Control>e"]);
    app.set_accels_for_action("app.undo", &["<Control>z"]);
    app.set_accels_for_action("app.redo", &["<Control><Shift>z"]);
    app.set_accels_for_action("app.copy", &["<Control>c"]);
    app.set_accels_for_action("app.save-as", &["<Control>s"]);
    app.set_accels_for_action("app.settings", &["<Control>comma"]);
    app.set_accels_for_action("app.extract-text", &["<Control><Shift>o"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    // Arguments are parsed by our own CLI; do not let GApplication interpret
    // application-specific flags such as `--capture`.
    app.run_with_args::<&str>(&["snipunix"])
}

fn activate(app: &adw::Application, cli: Cli) {
    if let Err(error) = adw::init() {
        tracing::error!(%error, "failed to initialise libadwaita");
    }

    let state = build_window(app, cli);
    install_actions(&state);
    state.window.present();

    setup_tray(&state);

    let launch_target = state.launch_target.get();
    let register_shortcuts_on_start = state.register_shortcuts_on_start.get()
        || state.settings.borrow().shortcuts.register_on_startup;
    if register_shortcuts_on_start {
        let state = Rc::clone(&state);
        glib::idle_add_local_once(move || register_shortcuts(&state));
    }
    if let Some(target) = launch_target {
        let state = Rc::clone(&state);
        glib::idle_add_local_once(move || start_capture(&state, target));
    } else if !state.settings.borrow().onboarding_complete {
        let settings = Rc::clone(&state.settings);
        let path = state.settings_path.clone();
        let parent: gtk::Window = state.window.clone().upcast();
        let weak = Rc::downgrade(&state);
        glib::idle_add_local_once(move || {
            onboarding::show(
                settings,
                path,
                &parent,
                Rc::new(move || {
                    if let Some(state) = weak.upgrade() {
                        register_shortcuts(&state);
                    }
                }),
                Rc::new(open_keyboard_settings),
            );
        });
    }
}

struct AppState {
    app: adw::Application,
    window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    stack: gtk::Stack,
    picture: gtk::Picture,
    status: gtk::Label,
    spinner: gtk::Spinner,
    busy: Cell<bool>,
    overlay_active: Cell<bool>,
    pending_output: RefCell<Option<PathBuf>>,
    exit_after: Cell<bool>,
    launch_target: Cell<Option<CaptureTarget>>,
    pending_window: Cell<Option<u32>>,
    backend_preference: Cell<snipunix_capture::BackendKind>,
    register_shortcuts_on_start: Cell<bool>,
    shortcut_registered: Cell<bool>,
    shortcut_stop: RefCell<Option<async_channel::Sender<()>>>,
    x11_hotkeys: RefCell<Option<snipunix_capture::x11_hotkey::X11HotkeyService>>,
    tray: RefCell<Option<tray::TrayHandle>>,
    settings: Rc<RefCell<Settings>>,
    settings_path: PathBuf,
    current: RefCell<Option<CapturedImage>>,
    last_region: RefCell<Option<RegionContext>>,
    clipboard_owned: Cell<bool>,
    delay: Cell<u32>,
    preview_bar: gtk::Box,
    copy_button: gtk::Button,
    save_button: gtk::Button,
    annotate_button: gtk::Button,
    adjust_button: gtk::Button,
    editor: RefCell<Option<Rc<EditorView>>>,
}

#[derive(Clone)]
struct RegionContext {
    full: CapturedImage,
    logical: LogicalRect,
    monitors: Vec<PixelMonitor>,
}

fn persist_settings(state: &AppState) {
    let settings = state.settings.borrow().clone();
    if let Err(error) = settings.save(&state.settings_path) {
        tracing::warn!(%error, path = %state.settings_path.display(), "could not save settings");
    }
}

impl AppState {
    fn set_status(&self, message: impl AsRef<str>) {
        self.status.set_label(message.as_ref());
    }

    fn toast(&self, message: impl AsRef<str>) {
        // Portal-provided trigger descriptions contain sequences such as
        // `<Control>s`; AdwToast renders its title as markup, so escape them.
        let escaped = glib::markup_escape_text(message.as_ref());
        self.toast_overlay
            .add_toast(adw::Toast::new(escaped.as_str()));
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        self.spinner.set_visible(busy);
        if busy {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }
    }

    fn display(&self, image: &CapturedImage) {
        let texture = texture_from_image(image);
        self.picture.set_paintable(Some(&texture));
        self.stack.set_visible_child_name("image");
    }

    fn quit_if_requested(&self) {
        if self.exit_after.get() {
            self.app.quit();
        }
    }
}

fn build_window(app: &adw::Application, cli: Cli) -> Rc<AppState> {
    let settings_path = Settings::default_path();
    let mut loaded = Settings::load(&settings_path).unwrap_or_else(|error| {
        tracing::warn!(%error, path = %settings_path.display(), "could not load settings; using defaults");
        Settings::default()
    });
    if cli.delay > 0 {
        loaded.capture.delay = cli.delay;
    }
    if cli.include_cursor {
        loaded.capture.include_cursor = true;
    }

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Snipunix")
        .default_width(1100)
        .default_height(760)
        .build();

    let header = adw::HeaderBar::new();
    let title = adw::WindowTitle::new("Snipunix", "Screenshot. Annotate. Share.");
    header.set_title_widget(Some(&title));

    let menu = gio::Menu::new();
    menu.append(Some("Capture Window…"), Some("app.capture-window"));
    menu.append(
        Some("Register Global Shortcuts"),
        Some("app.register-shortcuts"),
    );
    menu.append(Some("Settings…"), Some("app.settings"));
    menu.append(Some("Portal Information"), Some("app.portal-info"));
    menu.append(Some("About Snipunix"), Some("app.about"));
    menu.append(Some("Quit"), Some("app.quit"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Main menu")
        .build();
    header.pack_end(&menu_button);

    let toast_overlay = adw::ToastOverlay::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let action_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    action_bar.set_margin_top(12);
    action_bar.set_margin_bottom(6);
    action_bar.set_margin_start(12);
    action_bar.set_margin_end(12);
    let region_button = gtk::Button::with_label("Capture Region");
    region_button.add_css_class("suggested-action");
    region_button.set_action_name(Some("app.capture-region"));
    region_button.set_tooltip_text(Some("Capture the screen and select a rectangular region"));
    let screen_button = gtk::Button::with_label("Capture Screen");
    screen_button.set_action_name(Some("app.capture-screen"));
    screen_button.set_tooltip_text(Some("Capture the entire desktop"));
    action_bar.append(&region_button);
    action_bar.append(&screen_button);

    let delay_label = gtk::Label::new(Some("Delay"));
    delay_label.add_css_class("dim-label");
    delay_label.set_margin_start(12);
    let delay_spin = gtk::SpinButton::with_range(0.0, 10.0, 1.0);
    delay_spin.set_value(f64::from(loaded.capture.delay));
    delay_spin.set_tooltip_text(Some("Seconds to wait before freezing the screen"));
    delay_spin.set_width_chars(2);
    action_bar.append(&delay_label);
    action_bar.append(&delay_spin);
    content.append(&action_bar);

    let preview_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    preview_bar.set_margin_start(12);
    preview_bar.set_margin_end(12);
    preview_bar.set_margin_bottom(6);
    let copy_button = gtk::Button::with_label("Copy");
    copy_button.set_action_name(Some("app.copy"));
    copy_button.set_tooltip_text(Some("Copy the captured image to the clipboard"));
    let save_button = gtk::Button::with_label("Save As…");
    save_button.set_action_name(Some("app.save-as"));
    save_button.set_tooltip_text(Some("Save the captured image as a PNG"));
    let adjust_button = gtk::Button::with_label("Adjust Region");
    adjust_button.set_action_name(Some("app.adjust"));
    adjust_button.set_sensitive(false);
    let annotate_button = gtk::Button::with_label("Annotate");
    annotate_button.add_css_class("suggested-action");
    annotate_button.set_action_name(Some("app.annotate"));
    annotate_button.set_tooltip_text(Some("Open the annotation editor for the latest capture"));
    annotate_button.set_sensitive(false);
    copy_button.set_sensitive(false);
    save_button.set_sensitive(false);
    adjust_button.set_tooltip_text(Some(
        "Reopen the selection overlay to adjust the last region",
    ));
    preview_bar.append(&copy_button);
    preview_bar.append(&save_button);
    preview_bar.append(&annotate_button);
    preview_bar.append(&adjust_button);
    preview_bar.set_visible(false);

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .vexpand(true)
        .build();

    let empty_page = adw::StatusPage::builder()
        .icon_name("camera-photo-symbolic")
        .title("Screenshot. Annotate. Share.")
        .description(
            "Capture a region or the entire screen through the desktop portal. \
             Images stay in memory and never leave this device.",
        )
        .build();
    let (empty_label, empty_target) = match loaded.capture.default_mode {
        CaptureMode::Region => ("Capture Region", "app.capture-region"),
        CaptureMode::Screen => ("Capture Screen", "app.capture-screen"),
    };
    let empty_action = gtk::Button::with_label(empty_label);
    empty_action.add_css_class("suggested-action");
    empty_action.add_css_class("pill");
    empty_action.set_action_name(Some(empty_target));
    empty_page.set_child(Some(&empty_action));
    stack.add_named(&empty_page, Some("empty"));

    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .can_shrink(true)
        .hexpand(true)
        .vexpand(true)
        .build();
    let image_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    image_box.set_margin_top(6);
    image_box.set_margin_bottom(6);
    image_box.set_margin_start(12);
    image_box.set_margin_end(12);
    image_box.append(&picture);
    stack.add_named(&image_box, Some("image"));
    stack.set_visible_child_name("empty");
    content.append(&stack);
    content.append(&preview_bar);

    let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status_box.set_margin_top(4);
    status_box.set_margin_bottom(8);
    status_box.set_margin_start(12);
    status_box.set_margin_end(12);
    let status = gtk::Label::new(Some("Ready"));
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.add_css_class("dim-label");
    status.add_css_class("caption");
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    status_box.append(&status);
    status_box.append(&spinner);
    content.append(&status_box);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));
    toast_overlay.set_child(Some(&toolbar_view));
    window.set_content(Some(&toast_overlay));

    let initial_delay = loaded.capture.delay;
    let state = Rc::new(AppState {
        app: app.clone(),
        window,
        toast_overlay,
        stack,
        picture,
        status,
        spinner,
        busy: Cell::new(false),
        overlay_active: Cell::new(false),
        pending_output: RefCell::new(cli.output),
        exit_after: Cell::new(cli.exit_after),
        launch_target: Cell::new(cli.target),
        pending_window: Cell::new(cli.window),
        backend_preference: Cell::new(cli.backend),
        register_shortcuts_on_start: Cell::new(cli.register_shortcuts),
        shortcut_registered: Cell::new(false),
        shortcut_stop: RefCell::new(None),
        x11_hotkeys: RefCell::new(None),
        tray: RefCell::new(None),
        settings: Rc::new(RefCell::new(loaded)),
        settings_path,
        current: RefCell::new(None),
        last_region: RefCell::new(None),
        clipboard_owned: Cell::new(false),
        delay: Cell::new(initial_delay),
        preview_bar,
        copy_button,
        save_button,
        annotate_button,
        adjust_button,
        editor: RefCell::new(None),
    });

    let delay_state = Rc::downgrade(&state);
    delay_spin.connect_value_changed(move |spin| {
        if let Some(state) = delay_state.upgrade() {
            let delay = spin.value_as_int().clamp(0, 10) as u32;
            state.delay.set(delay);
            state.settings.borrow_mut().capture.delay = delay;
            persist_settings(&state);
        }
    });

    // Keep the process alive while Snipunix owns the clipboard. The image is
    // served from this process; closing the window only hides it. Quit remains
    // available from the menu or Ctrl+Q.
    let close_state = Rc::downgrade(&state);
    state.window.connect_close_request(move |window| {
        if let Some(state) = close_state.upgrade() {
            if state.clipboard_owned.get() && !state.exit_after.get() {
                window.set_visible(false);
                state.set_status(
                    "Snipunix is still running so the copied image stays on the clipboard.                      Use Quit to exit.",
                );
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });

    state
}

fn install_actions(state: &Rc<AppState>) {
    let app = &state.app;

    add_action(app, "capture-region", {
        let state = Rc::clone(state);
        move || start_capture(&state, CaptureTarget::Region)
    });
    add_action(app, "capture-screen", {
        let state = Rc::clone(state);
        move || start_capture(&state, CaptureTarget::Screen)
    });
    add_action(app, "capture-window", {
        let state = Rc::clone(state);
        move || start_capture(&state, CaptureTarget::Window)
    });
    add_action(app, "register-shortcuts", {
        let state = Rc::clone(state);
        move || register_shortcuts(&state)
    });
    add_action(app, "settings", {
        let state = Rc::clone(state);
        move || open_settings(&state)
    });
    add_action(app, "extract-text", {
        let state = Rc::clone(state);
        move || {
            if let Some(editor) = state.editor.borrow().as_ref().cloned() {
                editor.extract_text();
            } else {
                state.toast("Open the annotation editor before extracting text.");
            }
        }
    });
    add_action(app, "portal-info", {
        let state = Rc::clone(state);
        move || show_portal_info(&state)
    });
    add_action(app, "about", {
        let state = Rc::clone(state);
        move || show_about(&state)
    });
    add_action(app, "copy", {
        let state = Rc::clone(state);
        move || copy_current(&state)
    });
    add_action(app, "save-as", {
        let state = Rc::clone(state);
        move || save_as(&state)
    });
    add_action(app, "adjust", {
        let state = Rc::clone(state);
        move || adjust_region(&state)
    });
    add_action(app, "annotate", {
        let state = Rc::clone(state);
        move || open_editor(&state)
    });
    add_action(app, "undo", {
        let state = Rc::clone(state);
        move || {
            if let Some(editor) = state.editor.borrow().as_ref().cloned() {
                editor.undo();
            }
        }
    });
    add_action(app, "redo", {
        let state = Rc::clone(state);
        move || {
            if let Some(editor) = state.editor.borrow().as_ref().cloned() {
                editor.redo();
            }
        }
    });
    add_action(app, "quit", {
        let app = app.clone();
        move || app.quit()
    });
}

fn add_action<F>(app: &adw::Application, name: &str, callback: F)
where
    F: Fn() + 'static,
{
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_action, _parameter| callback());
    app.add_action(&action);
}

fn start_capture(state: &Rc<AppState>, target: CaptureTarget) {
    if state.busy.get() || state.overlay_active.get() {
        state.toast("A capture is already in progress.");
        return;
    }
    if target == CaptureTarget::Window
        && state.backend_preference.get() != snipunix_capture::BackendKind::X11
        && !snipunix_capture::is_x11_session()
    {
        state.toast("Window capture needs an X11 session or the x11 backend.");
        state.quit_if_requested();
        return;
    }
    if target == CaptureTarget::Window && state.pending_window.get().is_none() {
        show_window_picker(state);
        return;
    }
    let delay = state.delay.get();
    if delay == 0 {
        request_capture(state, target);
        return;
    }

    state.set_busy(true);
    state.set_status(format!("Capturing in {delay} seconds…"));
    let countdown_state = Rc::clone(state);
    let reduced_motion = state.settings.borrow().appearance.reduced_motion;
    let result = start_countdown(&state.app, delay, reduced_motion, move |completed| {
        countdown_state.set_busy(false);
        if completed {
            request_capture(&countdown_state, target);
        } else {
            countdown_state.set_status("Capture cancelled.");
            countdown_state.quit_if_requested();
        }
    });
    if let Err(error) = result {
        state.set_busy(false);
        let message = format!("Could not start the countdown: {error}");
        state.set_status(&message);
        state.toast(&message);
    }
}

/// X11 window picker used by **Capture Window** when no ID was supplied on the
/// command line.
fn show_window_picker(state: &Rc<AppState>) {
    let backend = state.backend_preference.get();
    if backend == snipunix_capture::BackendKind::Portal || !snipunix_capture::is_x11_session() {
        state.toast("Window capture needs an X11 session or the x11 backend.");
        return;
    }
    let windows = match snipunix_capture::X11Capture::new().list_windows() {
        Ok(windows) => windows,
        Err(error) => {
            state.toast(format!("Could not list windows: {error}"));
            return;
        }
    };
    if windows.is_empty() {
        state.toast("No capturable windows were found.");
        return;
    }

    let dialog = adw::Window::builder()
        .title("Capture Window")
        .transient_for(&state.window)
        .modal(true)
        .default_width(620)
        .default_height(520)
        .build();
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("boxed-list");
    for window in &windows {
        let row = adw::ActionRow::new();
        row.set_title(&glib::markup_escape_text(&window.title));
        let id = format!("{:#x}", window.id);
        row.set_subtitle(&format!(
            "{} · {}×{} · {}",
            if window.class.is_empty() {
                "window"
            } else {
                &window.class
            },
            window.width,
            window.height,
            id,
        ));
        row.set_activatable(true);
        list.append(&row);
    }
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    dialog.set_content(Some(&scrolled));

    let ids: Vec<u32> = windows.iter().map(|window| window.id).collect();
    let picked = Rc::new(Cell::new(false));
    let picker_state = Rc::clone(state);
    let picker_dialog = dialog.clone();
    let picker_picked = Rc::clone(&picked);
    list.connect_row_activated(move |_list, row| {
        let index = row.index();
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if let Some(id) = ids.get(index) {
            picker_picked.set(true);
            picker_state.pending_window.set(Some(*id));
            picker_dialog.close();
            start_capture(&picker_state, CaptureTarget::Window);
        }
    });

    let close_state = Rc::clone(state);
    dialog.connect_close_request(move |_dialog| {
        if !picked.get() {
            close_state.set_status("Window capture cancelled.");
            close_state.quit_if_requested();
        }
        glib::Propagation::Proceed
    });
    dialog.present();
}

fn request_capture(state: &Rc<AppState>, target: CaptureTarget) {
    if state.busy.get() || state.overlay_active.get() {
        state.toast("A capture is already in progress.");
        return;
    }
    state.set_busy(true);
    state.set_status("Capturing the screen…");
    // Hide the app window so it is not part of its own screenshot. The picture
    // is shown again once the capture or overlay session finishes.
    state.window.set_visible(false);

    let state = Rc::clone(state);
    glib::spawn_future_local(async move {
        // Let the compositor remove our window before the portal captures.
        glib::timeout_future(std::time::Duration::from_millis(180)).await;
        // The window is hidden at this point, so it cannot be used as a portal
        // parent (its xdg-foreign handle would be gone); the portal dialog is
        // still shown by the compositor when permission is required.
        let backend = AutoCapture::new(state.backend_preference.get());
        // Read the setting before awaiting so no RefCell borrow is held across
        // the capture future.
        let include_cursor = state.settings.borrow().capture.include_cursor;
        let result = backend
            .capture(CaptureOptions {
                interactive: false,
                parent: None,
                window: match target {
                    CaptureTarget::Window => state.pending_window.get(),
                    _ => None,
                },
                include_cursor,
            })
            .await;
        state.set_busy(false);
        match result {
            Ok(frame) => handle_capture(&state, frame, target),
            Err(error) => handle_capture_error(&state, &error),
        }
    });
}

fn handle_capture(state: &Rc<AppState>, frame: CaptureFrame, target: CaptureTarget) {
    state.window.present();
    close_editor(state);
    match target {
        CaptureTarget::Screen => {
            let image = frame.image;
            state.display(&image);
            state.current.replace(Some(image.clone()));
            state.last_region.borrow_mut().take();
            state.preview_bar.set_visible(true);
            state.copy_button.set_sensitive(true);
            state.save_button.set_sensitive(true);
            state.annotate_button.set_sensitive(true);
            state.adjust_button.set_sensitive(false);
            let copied = state.settings.borrow().capture.copy_after_capture
                && copy_to_clipboard(state, &image);
            state.set_status(if copied {
                format!(
                    "Captured the full desktop ({} × {}) and copied it to the clipboard.",
                    image.width(),
                    image.height()
                )
            } else {
                format!(
                    "Captured the full desktop ({} × {}).",
                    image.width(),
                    image.height()
                )
            });
            save_after_capture(state, &image);
            state.quit_if_requested();
        }
        CaptureTarget::Region => {
            let initial = if state.settings.borrow().capture.remember_last_region {
                state
                    .last_region
                    .borrow()
                    .as_ref()
                    .map(|context| context.logical)
            } else {
                None
            };
            begin_overlay(state, frame, overlay_options(state, initial));
        }
        CaptureTarget::Window => {
            // Window captures are already cropped by the backend.
            state.pending_window.set(None);
            let image = frame.image;
            state.display(&image);
            state.current.replace(Some(image.clone()));
            state.last_region.borrow_mut().take();
            state.preview_bar.set_visible(true);
            state.copy_button.set_sensitive(true);
            state.save_button.set_sensitive(true);
            state.annotate_button.set_sensitive(true);
            state.adjust_button.set_sensitive(false);
            let copied = state.settings.borrow().capture.copy_after_capture
                && copy_to_clipboard(state, &image);
            state.set_status(if copied {
                format!(
                    "Captured the window ({} × {}) and copied it to the clipboard.",
                    image.width(),
                    image.height()
                )
            } else {
                format!(
                    "Captured the window ({} × {}).",
                    image.width(),
                    image.height()
                )
            });
            save_after_capture(state, &image);
            state.quit_if_requested();
        }
    }
}

fn handle_capture_error(state: &Rc<AppState>, error: &CaptureError) {
    state.window.present();
    let message = match error {
        CaptureError::Cancelled | CaptureError::CancelledWithDetail(_) => {
            "Capture cancelled.".to_string()
        }
        CaptureError::PermissionDenied | CaptureError::PermissionDeniedWithDetail(_) => {
            "Screenshot permission was denied. Open Settings → Privacy → Screenshots, \
             allow Snipunix, then try again."
                .to_string()
        }
        CaptureError::PortalUnavailable(_) => {
            "No screenshot portal is available. Install xdg-desktop-portal and a desktop \
             backend, then try again."
                .to_string()
        }
        CaptureError::X11Unavailable(detail) => {
            format!("X11 capture is unavailable: {detail}")
        }
        CaptureError::X11(detail) => format!("X11 capture failed: {detail}"),
        CaptureError::Unsupported(detail) => format!("Capture is not supported: {detail}"),
        CaptureError::Timeout(duration) => {
            format!("The screenshot portal did not respond within {duration:?}.")
        }
        other => format!("Capture failed: {other}"),
    };
    state.set_status(&message);
    if matches!(
        error,
        CaptureError::PermissionDenied | CaptureError::PermissionDeniedWithDetail(_)
    ) {
        let parent: gtk::Window = state.window.clone().upcast();
        settings_ui::show_permission_help(&parent, &message);
    } else if !matches!(
        error,
        CaptureError::Cancelled | CaptureError::CancelledWithDetail(_)
    ) {
        state.toast(&message);
    }
    state.quit_if_requested();
}

fn begin_overlay(state: &Rc<AppState>, frame: CaptureFrame, options: OverlayOptions) {
    let monitors = match collect_monitors() {
        Ok(monitors) => monitors,
        Err(error) => {
            state.window.present();
            state.toast(format!("Could not read the monitor layout: {error}"));
            return;
        }
    };
    let layout = match layout_from_monitors(&monitors) {
        Ok(layout) => layout,
        Err(error) => {
            state.window.present();
            state.toast(format!("Could not build the monitor layout: {error}"));
            return;
        }
    };
    let pixel_monitors = frame.monitors;
    let full = frame.image;
    let mapping = match capture_mapping(
        layout.bounding_box(),
        full.size(),
        &pixel_monitors,
        &monitors,
    ) {
        Ok(mapping) => mapping,
        Err(error) => {
            state.window.present();
            state.toast(format!("Could not map the captured image: {error}"));
            return;
        }
    };
    let frames = overlay_frames(&full, &monitors, &mapping);

    state.overlay_active.set(true);
    state.set_status(if options.auto_commit_on_release {
        "Drag to select a region; release captures it. Escape cancels."
    } else {
        "Drag the handles or move the selection, then press Enter or Ctrl+C to copy. \
         Escape clears the selection."
    });

    let state_done = Rc::clone(state);
    let presented = present(
        &state.app,
        frames,
        mapping,
        monitors,
        options,
        move |result| {
            state_done.overlay_active.set(false);
            state_done.window.present();
            match result {
                OverlayResult::Selected { logical, image } => {
                    commit_region(&state_done, &full, &pixel_monitors, logical, image);
                }
                OverlayResult::Cancelled => {
                    state_done.set_status(if options.auto_commit_on_release {
                        "Selection cancelled."
                    } else {
                        "Adjustment cancelled."
                    });
                }
            }
            state_done.quit_if_requested();
        },
    );

    if let Err(error) = presented {
        state.overlay_active.set(false);
        state.window.present();
        state.toast(format!("Could not open the selection overlay: {error}"));
    }
}

/// Builds the coordinate mapping. Exact per-monitor pixel rectangles from the
/// backend are matched to GDK monitors by connector name (falling back to
/// list order); without them the mapping is uniform.
fn capture_mapping(
    desktop: LogicalRect,
    image: snipunix_core::ImageSize,
    pixel_monitors: &[PixelMonitor],
    monitors: &[snipunix_overlay::OverlayMonitor],
) -> Result<CaptureMapping, MappingError> {
    if !pixel_monitors.is_empty() && pixel_monitors.len() == monitors.len() {
        let mut per_monitor = Vec::with_capacity(monitors.len());
        for overlay in monitors {
            let matched = pixel_monitors
                .iter()
                .find(|pixel| pixel.name.as_deref() == Some(overlay.geometry.name.as_str()))
                .or_else(|| pixel_monitors.get(overlay.geometry.id as usize));
            let Some(pixel) = matched else {
                per_monitor.clear();
                break;
            };
            per_monitor.push(MonitorImage {
                id: overlay.geometry.id,
                logical: overlay.geometry.rect(),
                image: pixel.rect,
            });
        }
        if per_monitor.len() == monitors.len() {
            if let Ok(mapping) = CaptureMapping::with_monitors(desktop, image, per_monitor) {
                tracing::debug!(
                    monitors = monitors.len(),
                    "using exact per-monitor capture mapping"
                );
                return Ok(mapping);
            }
            tracing::warn!("per-monitor rectangles were invalid; using a uniform mapping");
        }
    }
    CaptureMapping::new(desktop, image)
}

fn overlay_frames(
    full: &CapturedImage,
    monitors: &[snipunix_overlay::OverlayMonitor],
    mapping: &CaptureMapping,
) -> OverlayFrames {
    let full_texture = texture_from_image(full);
    let mut per_monitor = Vec::with_capacity(monitors.len());
    for monitor in monitors {
        let texture = mapping
            .image_rect_for_monitor(&monitor.geometry)
            .and_then(|rect| full.crop(rect).ok())
            .map(|image| texture_from_image(&image));
        per_monitor.push(texture);
    }
    OverlayFrames {
        full: full_texture,
        per_monitor,
    }
}

fn commit_region(
    state: &Rc<AppState>,
    full: &CapturedImage,
    monitors: &[PixelMonitor],
    logical: LogicalRect,
    image: ImageRect,
) {
    tracing::debug!(?logical, ?image, "cropping selected region");
    match full.crop(image) {
        Ok(cropped) => {
            tracing::debug!(
                width = cropped.width(),
                height = cropped.height(),
                "cropped selected region"
            );
            state.window.present();
            close_editor(state);
            state.display(&cropped);
            state.current.replace(Some(cropped.clone()));
            state.last_region.replace(Some(RegionContext {
                full: full.clone(),
                logical,
                monitors: monitors.to_vec(),
            }));
            state.preview_bar.set_visible(true);
            state.copy_button.set_sensitive(true);
            state.save_button.set_sensitive(true);
            state.annotate_button.set_sensitive(true);
            state.adjust_button.set_sensitive(true);
            let copied = state.settings.borrow().capture.copy_after_capture
                && copy_to_clipboard(state, &cropped);
            state.set_status(if copied {
                format!(
                    "Selected region ({} × {}) and copied it to the clipboard.",
                    cropped.width(),
                    cropped.height()
                )
            } else {
                format!(
                    "Selected region ({} × {}).",
                    cropped.width(),
                    cropped.height()
                )
            });
            save_after_capture(state, &cropped);
        }
        Err(error) => {
            tracing::warn!(%error, "could not crop the region");
            state.toast(format!("Could not crop the region: {error}"));
        }
    }
}

fn copy_to_clipboard(state: &AppState, image: &CapturedImage) -> bool {
    let Some(display) = gdk::Display::default() else {
        state.toast("Could not access the display clipboard.");
        return false;
    };
    let texture = texture_from_image(image);
    display.clipboard().set_texture(&texture);
    state.clipboard_owned.set(true);
    tracing::info!(
        width = image.width(),
        height = image.height(),
        "copied image to clipboard"
    );
    true
}

fn open_editor(state: &Rc<AppState>) {
    if state.editor.borrow().is_some() {
        state.stack.set_visible_child_name("editor");
        return;
    }
    let Some(image) = state.current.borrow().clone() else {
        state.toast("Capture a screen or region before annotating.");
        return;
    };

    let status_weak = Rc::downgrade(state);
    let toast_weak = Rc::downgrade(state);
    let close_weak = Rc::downgrade(state);
    let copied_weak = Rc::downgrade(state);
    let callbacks = editor::Callbacks {
        status: Rc::new(move |message: &str| {
            if let Some(state) = status_weak.upgrade() {
                state.set_status(message);
            }
        }),
        toast: Rc::new(move |message: &str| {
            if let Some(state) = toast_weak.upgrade() {
                state.toast(message);
            }
        }),
        close: Rc::new(move || {
            if let Some(state) = close_weak.upgrade() {
                close_editor(&state);
            }
        }),
        copied: Rc::new(move || {
            if let Some(state) = copied_weak.upgrade() {
                state.clipboard_owned.set(true);
            }
        }),
    };

    let parent: gtk::Window = state.window.clone().upcast();
    match EditorView::new(image, parent, callbacks, Rc::clone(&state.settings)) {
        Ok(view) => {
            let view = Rc::new(view);
            state.stack.add_named(&view.root, Some("editor"));
            state.stack.set_visible_child_name("editor");
            state.preview_bar.set_visible(false);
            state.editor.replace(Some(view));
            state.set_status(
                "Annotation editor ready. Scroll to zoom, middle-drag to pan, Ctrl+Z to undo.",
            );
        }
        Err(error) => {
            state.toast(format!("Could not open the annotation editor: {error}"));
        }
    }
}

fn close_editor(state: &AppState) {
    let Some(view) = state.editor.borrow_mut().take() else {
        return;
    };
    state.stack.remove(&view.root);
    if state.current.borrow().is_some() {
        state.stack.set_visible_child_name("image");
        state.preview_bar.set_visible(true);
    } else {
        state.stack.set_visible_child_name("empty");
    }
}

fn copy_current(state: &Rc<AppState>) {
    if let Some(editor) = state.editor.borrow().as_ref().cloned() {
        editor.copy_annotated();
        return;
    }
    let Some(image) = state.current.borrow().clone() else {
        state.toast("There is no captured image to copy yet.");
        return;
    };
    if copy_to_clipboard(state, &image) {
        state.set_status(format!(
            "Copied {} × {} to the clipboard.",
            image.width(),
            image.height()
        ));
    }
}

fn save_as(state: &Rc<AppState>) {
    if let Some(editor) = state.editor.borrow().as_ref().cloned() {
        editor.open_export_dialog();
        return;
    }
    let Some(image) = state.current.borrow().clone() else {
        state.toast("There is no captured image to save yet.");
        return;
    };
    let (format, quality) = {
        let settings = state.settings.borrow();
        (
            export_format(settings.files.format),
            settings.files.jpeg_quality,
        )
    };
    let dialog = gtk::FileDialog::builder()
        .title("Save screenshot")
        .initial_name(default_file_name(format))
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

    let state = Rc::clone(state);
    let parent = state.window.clone();
    dialog.save(
        Some(&parent),
        None::<&gio::Cancellable>,
        move |result| match result {
            Ok(file) => {
                let Some(path) = file.path() else {
                    state.toast("The selected location is not a local file.");
                    return;
                };
                let path = with_format_extension(path, format);
                match encode_capture(&image, format, quality)
                    .and_then(|bytes| {
                        std::fs::write(&path, bytes).map_err(|error| error.to_string())
                    })
                {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), format = format.label(), "saved capture");
                        let message = format!("Saved {} to {}", format.label(), path.display());
                        state.set_status(&message);
                        state.toast(&message);
                    }
                    Err(error) => {
                        let message =
                            format!("Could not save {} to {}: {error}", format.label(), path.display());
                        state.toast(&message);
                    }
                }
            }
            Err(error) => {
                if !error.matches(gtk::DialogError::Dismissed) {
                    state.toast(format!("Could not choose a save location: {error}"));
                }
            }
        },
    );
}

fn export_format(format: snipunix_settings::SaveFormat) -> ExportFormat {
    match format {
        snipunix_settings::SaveFormat::Png => ExportFormat::Png,
        snipunix_settings::SaveFormat::Jpeg => ExportFormat::Jpeg,
        snipunix_settings::SaveFormat::Webp => ExportFormat::WebP,
    }
}

fn with_format_extension(mut path: PathBuf, format: ExportFormat) -> PathBuf {
    if path.extension().and_then(|extension| extension.to_str()) != Some(format.extension()) {
        path.set_extension(format.extension());
    }
    path
}

fn encode_capture(
    image: &CapturedImage,
    format: ExportFormat,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let rendered = RenderedImage::new(image.width(), image.height(), image.rgba().to_vec())
        .map_err(|error| error.to_string())?;
    snipunix_annotate::encode(&rendered, format, quality).map_err(|error| error.to_string())
}

fn default_file_name(format: ExportFormat) -> String {
    let stamp = glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format("%Y%m%d-%H%M%S").ok())
        .map_or_else(|| "screenshot".to_string(), |value| value.to_string());
    format!("snipunix-{stamp}.{}", format.extension())
}

fn adjust_region(state: &Rc<AppState>) {
    if state.busy.get() || state.overlay_active.get() {
        state.toast("A capture is already in progress.");
        return;
    }
    let Some(context) = state.last_region.borrow().clone() else {
        state.toast("There is no region to adjust.");
        return;
    };
    let options = overlay_options(state, Some(context.logical));
    begin_overlay(
        state,
        CaptureFrame {
            image: context.full,
            monitors: context.monitors,
        },
        OverlayOptions {
            auto_commit_on_release: false,
            ..options
        },
    );
}

fn overlay_options(state: &AppState, initial: Option<LogicalRect>) -> OverlayOptions {
    let settings = state.settings.borrow();
    OverlayOptions {
        auto_commit_on_release: true,
        initial_selection: initial,
        magnifier: settings.appearance.show_magnifier,
        dim_strength: settings.appearance.dim_strength as f32,
    }
}

fn save_after_capture(state: &AppState, image: &CapturedImage) -> bool {
    if save_pending(state, image) {
        return true;
    }
    if !state.settings.borrow().files.auto_save {
        return false;
    }
    save_auto(state, image)
}

fn save_auto(state: &AppState, image: &CapturedImage) -> bool {
    let (directory, template, format, quality) = {
        let settings = state.settings.borrow();
        (
            settings.files.directory_or_default(),
            settings.files.filename_template.clone(),
            settings.files.format,
            settings.files.jpeg_quality,
        )
    };
    let expanded = glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format(&template).ok())
        .map_or_else(|| "Screenshot".to_string(), |value| value.to_string())
        .replace('/', "-");
    let extension = format.extension();
    let mut name = if expanded
        .to_ascii_lowercase()
        .ends_with(&format!(".{extension}"))
    {
        expanded
    } else {
        format!("{expanded}.{extension}")
    };
    let mut path = directory.join(&name);
    let mut suffix = 1;
    while path.exists() {
        let stem = name
            .rsplit_once('.')
            .map_or(name.as_str(), |(stem, _)| stem)
            .to_string();
        name = format!("{stem}-{suffix}.{extension}");
        path = directory.join(&name);
        suffix += 1;
        if suffix > 10_000 {
            state.toast("Could not find a free auto-save file name.");
            return false;
        }
    }
    let format = export_format(format);
    match encode_capture(image, format, quality).and_then(|bytes| {
        std::fs::create_dir_all(&directory)
            .map_err(|error| error.to_string())
            .and_then(|()| std::fs::write(&path, bytes).map_err(|error| error.to_string()))
    }) {
        Ok(()) => {
            let message = format!("Auto-saved to {}", path.display());
            tracing::info!(path = %path.display(), "auto-saved capture");
            state.toast(&message);
            true
        }
        Err(error) => {
            state.toast(format!("Could not auto-save {}: {error}", path.display()));
            false
        }
    }
}

fn save_pending(state: &AppState, image: &CapturedImage) -> bool {
    // Keep the path: an adjusted selection must overwrite the same file.
    let Some(path) = state.pending_output.borrow().clone() else {
        return false;
    };
    match image.save_png(&path) {
        Ok(()) => {
            tracing::info!(path = %path.display(), "saved PNG");
            let message = format!("Saved PNG to {}", path.display());
            state.set_status(&message);
            state.toast(&message);
            true
        }
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "could not save PNG");
            let message = format!("Could not save PNG to {}: {error}", path.display());
            state.toast(&message);
            false
        }
    }
}

fn texture_from_image(image: &CapturedImage) -> gdk::Texture {
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

fn register_shortcuts(state: &Rc<AppState>) {
    // A rebind starts a fresh portal session: stop the old listener first so
    // two sessions never fight over the same application ID.
    if let Some(stop) = state.shortcut_stop.borrow_mut().take() {
        let _ = stop.send_blocking(());
        state.shortcut_registered.set(false);
    }
    state.x11_hotkeys.borrow_mut().take();
    if state.shortcut_registered.get() {
        state.toast("Global shortcuts are already registered.");
        return;
    }
    state.set_status("Requesting global shortcut registration…");
    let (region_trigger, screen_trigger) = {
        let settings = state.settings.borrow();
        (
            settings.shortcuts.region_trigger.clone(),
            settings.shortcuts.screen_trigger.clone(),
        )
    };
    let (stop_sender, stop_receiver) = async_channel::bounded(1);
    state.shortcut_stop.replace(Some(stop_sender));

    let (sender, receiver) = async_channel::unbounded();
    let receiver_state = Rc::clone(state);
    glib::spawn_future_local(async move {
        while let Ok(event) = receiver.recv().await {
            handle_shortcut_event(&receiver_state, event);
        }
    });

    let window = state.window.clone();
    let fallback_state = Rc::clone(state);
    glib::spawn_future_local(async move {
        let parent = if window.is_realized() {
            ashpd::WindowIdentifier::from_native(&window).await
        } else {
            None
        };
        let error_sender = sender.clone();
        if let Err(error) = shortcuts::run(
            sender.clone(),
            parent,
            &region_trigger,
            &screen_trigger,
            stop_receiver,
        )
        .await
        {
            let message = shortcuts::describe_error(&error);
            tracing::warn!(%error, "global shortcut registration failed");
            if snipunix_capture::is_x11_session()
                && start_x11_shortcuts(&fallback_state, &sender, &region_trigger, &screen_trigger)
            {
                tracing::info!("using the X11 key-grab fallback for global shortcuts");
                return;
            }
            let _ = error_sender.send(ShortcutEvent::Failed(message)).await;
        }
    });
}

/// Grabs the configured triggers directly with X11 when the portal is
/// unavailable. Returns `true` when the grabs were installed.
fn start_x11_shortcuts(
    state: &Rc<AppState>,
    sender: &async_channel::Sender<ShortcutEvent>,
    region_trigger: &str,
    screen_trigger: &str,
) -> bool {
    let triggers = vec![region_trigger.to_string(), screen_trigger.to_string()];
    let hotkey_sender = sender.clone();
    let spawned = snipunix_capture::x11_hotkey::spawn(&triggers, move |index| {
        let id = if index == 0 {
            shortcuts::REGION_SHORTCUT_ID
        } else {
            shortcuts::SCREEN_SHORTCUT_ID
        };
        let _ = hotkey_sender.send_blocking(ShortcutEvent::Activated(id.to_string()));
    });
    match spawned {
        Ok(service) => {
            state.x11_hotkeys.replace(Some(service));
            state.shortcut_registered.set(true);
            let bound = vec![
                BoundShortcut {
                    id: shortcuts::REGION_SHORTCUT_ID.to_string(),
                    description: "Capture a screen region".to_string(),
                    trigger: region_trigger.to_string(),
                },
                BoundShortcut {
                    id: shortcuts::SCREEN_SHORTCUT_ID.to_string(),
                    description: "Capture the full screen".to_string(),
                    trigger: screen_trigger.to_string(),
                },
            ];
            let _ = sender.send_blocking(ShortcutEvent::Registered(bound));
            true
        }
        Err(error) => {
            tracing::warn!(%error, "X11 key-grab fallback failed");
            false
        }
    }
}

fn setup_tray(state: &Rc<AppState>) {
    let (sender, receiver) = std::sync::mpsc::channel();
    match tray::spawn(sender) {
        Ok(handle) => {
            state.tray.replace(Some(handle));
            tracing::debug!("tray service started");
        }
        Err(error) => {
            // Default GNOME has no StatusNotifierItem host unless the
            // AppIndicator extension is enabled. This is not an error state.
            tracing::debug!(%error, "tray is unavailable; continuing without it");
            return;
        }
    }
    let weak = Rc::downgrade(state);
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        while let Ok(command) = receiver.try_recv() {
            let Some(state) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            handle_tray_command(&state, command);
        }
        glib::ControlFlow::Continue
    });
}

fn handle_tray_command(state: &Rc<AppState>, command: TrayCommand) {
    match command {
        TrayCommand::CaptureRegion => {
            state.window.present();
            start_capture(state, CaptureTarget::Region);
        }
        TrayCommand::CaptureScreen => {
            state.window.present();
            start_capture(state, CaptureTarget::Screen);
        }
        TrayCommand::Show => state.window.present(),
        TrayCommand::Settings => open_settings(state),
        TrayCommand::Quit => state.app.quit(),
    }
}

fn handle_shortcut_event(state: &Rc<AppState>, event: ShortcutEvent) {
    match event {
        ShortcutEvent::Registered(bindings) => {
            state.shortcut_registered.set(true);
            tracing::info!(?bindings, "global shortcuts registered");
            let descriptions = bindings
                .iter()
                .map(|shortcut| {
                    format!(
                        "{} — {} ({})",
                        shortcut.id, shortcut.description, shortcut.trigger
                    )
                })
                .collect::<Vec<_>>();
            let summary = if descriptions.is_empty() {
                "Global shortcuts registered, but the portal returned no trigger description."
                    .to_string()
            } else {
                format!(
                    "Global shortcuts registered: {}. They stay active while Snipunix is running.",
                    descriptions.join("; ")
                )
            };
            let bound = descriptions.join("; ");
            {
                let mut settings = state.settings.borrow_mut();
                settings.shortcuts.bound_description = bound;
                settings.sanitize();
            }
            persist_settings(state);
            state.set_status(&summary);
            state.toast(&summary);
        }
        ShortcutEvent::Failed(message) => {
            state.set_status("Global shortcuts are not registered.");
            state.toast(&message);
            let parent: gtk::Window = state.window.clone().upcast();
            settings_ui::show_shortcut_help(&parent, &message);
        }
        ShortcutEvent::Activated(id) => {
            tracing::info!(shortcut = %id, "global shortcut activated");
            state.toast(format!("Global shortcut activated: {id}"));
            match id.as_str() {
                shortcuts::REGION_SHORTCUT_ID => start_capture(state, CaptureTarget::Region),
                shortcuts::SCREEN_SHORTCUT_ID => start_capture(state, CaptureTarget::Screen),
                unknown => tracing::warn!(shortcut = unknown, "unknown shortcut activated"),
            }
        }
        ShortcutEvent::Deactivated(id) => {
            state.toast(format!("Global shortcut released: {id}"));
        }
    }
}

fn show_portal_info(state: &Rc<AppState>) {
    let state = Rc::clone(state);
    glib::spawn_future_local(async move {
        match snipunix_capture::probe_portal().await {
            Ok(info) => {
                let targets = info.available_targets.map_or_else(
                    || "not available (Screenshot v2 backend)".to_string(),
                    |bits| format!("{bits:#x}"),
                );
                let message = format!(
                    "org.freedesktop.portal.Screenshot version {} · AvailableTargets: {}",
                    info.version, targets
                );
                state.set_status(&message);
                state.toast(&message);
            }
            Err(error) => {
                let message = format!("Could not query the screenshot portal: {error}");
                state.set_status(&message);
                state.toast(&message);
            }
        }
    });
}

fn open_settings(state: &Rc<AppState>) {
    let parent: gtk::Window = state.window.clone().upcast();
    let weak = Rc::downgrade(state);
    let saved = Rc::new(move || {
        if let Some(state) = weak.upgrade() {
            state.delay.set(state.settings.borrow().capture.delay);
        }
    });
    let weak = Rc::downgrade(state);
    let register = Rc::new(move || {
        if let Some(state) = weak.upgrade() {
            register_shortcuts(&state);
        }
    });
    settings_ui::present(
        Rc::clone(&state.settings),
        state.settings_path.clone(),
        &parent,
        settings_ui::Callbacks {
            on_saved: saved,
            register_shortcuts: register,
            open_keyboard_settings: Rc::new(open_keyboard_settings),
        },
    );
}

fn open_keyboard_settings() {
    if let Err(error) = settings_ui::open_keyboard_settings() {
        tracing::warn!(%error, "could not open the system keyboard settings");
    }
}

fn show_about(state: &AppState) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Snipunix")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Snipunix contributors")
        .comments("Screenshot. Annotate. Share.")
        .license_type(gtk::License::Gpl30)
        .build();
    dialog.present(Some(&state.window));
}
