//! GTK4 + Libadwaita application shell and capture flow.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use snipunix_capture::{
    CaptureBackend, CaptureError, CaptureOptions, CapturedImage, PortalScreenshot,
};
use snipunix_core::{CaptureMapping, ImageRect, LogicalRect};
use snipunix_overlay::{
    OverlayFrames, OverlayOptions, OverlayResult, collect_monitors, layout_from_monitors, present,
    start_countdown,
};

use crate::cli::{CaptureTarget, Cli};
use crate::shortcuts::{self, ShortcutEvent};

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
    app.set_accels_for_action("app.copy", &["<Control>c"]);
    app.set_accels_for_action("app.save-as", &["<Control>s"]);
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

    let launch_target = state.launch_target.get();
    let register_shortcuts_on_start = state.register_shortcuts_on_start.get();
    if register_shortcuts_on_start {
        let state = Rc::clone(&state);
        glib::idle_add_local_once(move || register_shortcuts(&state));
    }
    if let Some(target) = launch_target {
        let state = Rc::clone(&state);
        glib::idle_add_local_once(move || start_capture(&state, target));
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
    register_shortcuts_on_start: Cell<bool>,
    shortcut_registered: Cell<bool>,
    current: RefCell<Option<CapturedImage>>,
    last_region: RefCell<Option<RegionContext>>,
    clipboard_owned: Cell<bool>,
    delay: Cell<u32>,
    preview_bar: gtk::Box,
    copy_button: gtk::Button,
    save_button: gtk::Button,
    adjust_button: gtk::Button,
}

#[derive(Clone)]
struct RegionContext {
    full: CapturedImage,
    logical: LogicalRect,
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
    menu.append(
        Some("Register Global Shortcut (prototype)"),
        Some("app.register-shortcuts"),
    );
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
    delay_spin.set_value(f64::from(cli.delay));
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
    copy_button.set_sensitive(false);
    save_button.set_sensitive(false);
    adjust_button.set_tooltip_text(Some(
        "Reopen the selection overlay to adjust the last region",
    ));
    preview_bar.append(&copy_button);
    preview_bar.append(&save_button);
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
    let empty_action = gtk::Button::with_label("Capture Region");
    empty_action.add_css_class("suggested-action");
    empty_action.add_css_class("pill");
    empty_action.set_action_name(Some("app.capture-region"));
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
        register_shortcuts_on_start: Cell::new(cli.register_shortcuts),
        shortcut_registered: Cell::new(false),
        current: RefCell::new(None),
        last_region: RefCell::new(None),
        clipboard_owned: Cell::new(false),
        delay: Cell::new(cli.delay),
        preview_bar,
        copy_button,
        save_button,
        adjust_button,
    });

    let delay_state = Rc::downgrade(&state);
    delay_spin.connect_value_changed(move |spin| {
        if let Some(state) = delay_state.upgrade() {
            state.delay.set(spin.value_as_int().clamp(0, 10) as u32);
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
    add_action(app, "register-shortcuts", {
        let state = Rc::clone(state);
        move || register_shortcuts(&state)
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
    let delay = state.delay.get();
    if delay == 0 {
        request_capture(state, target);
        return;
    }

    state.set_busy(true);
    state.set_status(format!("Capturing in {delay} seconds…"));
    let countdown_state = Rc::clone(state);
    let result = start_countdown(&state.app, delay, move |completed| {
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

fn request_capture(state: &Rc<AppState>, target: CaptureTarget) {
    if state.busy.get() || state.overlay_active.get() {
        state.toast("A capture is already in progress.");
        return;
    }
    state.set_busy(true);
    state.set_status("Freezing the screen through the desktop portal…");
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
        let backend = PortalScreenshot::default();
        let result = backend
            .capture(CaptureOptions {
                interactive: false,
                parent: None,
            })
            .await;
        state.set_busy(false);
        match result {
            Ok(image) => handle_capture(&state, image, target),
            Err(error) => handle_capture_error(&state, &error),
        }
    });
}

fn handle_capture(state: &Rc<AppState>, image: CapturedImage, target: CaptureTarget) {
    state.window.present();
    match target {
        CaptureTarget::Screen => {
            state.display(&image);
            state.current.replace(Some(image.clone()));
            state.last_region.borrow_mut().take();
            state.preview_bar.set_visible(true);
            state.copy_button.set_sensitive(true);
            state.save_button.set_sensitive(true);
            state.adjust_button.set_sensitive(false);
            let copied = copy_to_clipboard(state, &image);
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
            save_pending(state, &image);
            state.quit_if_requested();
        }
        CaptureTarget::Region => {
            begin_overlay(state, image, OverlayOptions::default());
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
        CaptureError::Timeout(duration) => {
            format!("The screenshot portal did not respond within {duration:?}.")
        }
        other => format!("Capture failed: {other}"),
    };
    state.set_status(&message);
    if !matches!(
        error,
        CaptureError::Cancelled | CaptureError::CancelledWithDetail(_)
    ) {
        state.toast(&message);
    }
    state.quit_if_requested();
}

fn begin_overlay(state: &Rc<AppState>, full: CapturedImage, options: OverlayOptions) {
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
    let mapping = match CaptureMapping::new(layout.bounding_box(), full.size()) {
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
                    commit_region(&state_done, &full, logical, image);
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
            state.display(&cropped);
            state.current.replace(Some(cropped.clone()));
            state.last_region.replace(Some(RegionContext {
                full: full.clone(),
                logical,
            }));
            state.preview_bar.set_visible(true);
            state.copy_button.set_sensitive(true);
            state.save_button.set_sensitive(true);
            state.adjust_button.set_sensitive(true);
            let copied = copy_to_clipboard(state, &cropped);
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
            save_pending(state, &cropped);
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

fn copy_current(state: &Rc<AppState>) {
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
    let Some(image) = state.current.borrow().clone() else {
        state.toast("There is no captured image to save yet.");
        return;
    };
    let dialog = gtk::FileDialog::builder()
        .title("Save screenshot")
        .initial_name(default_file_name())
        .build();
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
                match image.save_png(&path) {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), "saved PNG");
                        let message = format!("Saved PNG to {}", path.display());
                        state.set_status(&message);
                        state.toast(&message);
                    }
                    Err(error) => {
                        let message = format!("Could not save PNG to {}: {error}", path.display());
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

fn default_file_name() -> String {
    let stamp = glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format("%Y%m%d-%H%M%S").ok())
        .map_or_else(|| "screenshot".to_string(), |value| value.to_string());
    format!("snipunix-{stamp}.png")
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
    begin_overlay(
        state,
        context.full,
        OverlayOptions {
            auto_commit_on_release: false,
            initial_selection: Some(context.logical),
            magnifier: true,
        },
    );
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
    if state.shortcut_registered.get() {
        state.toast("Global shortcuts are already registered.");
        return;
    }
    state.set_status("Requesting global shortcut registration…");

    let (sender, receiver) = async_channel::unbounded();
    let receiver_state = Rc::clone(state);
    glib::spawn_future_local(async move {
        while let Ok(event) = receiver.recv().await {
            handle_shortcut_event(&receiver_state, event);
        }
    });

    let window = state.window.clone();
    glib::spawn_future_local(async move {
        let parent = if window.is_realized() {
            ashpd::WindowIdentifier::from_native(&window).await
        } else {
            None
        };
        let error_sender = sender.clone();
        if let Err(error) = shortcuts::run(sender, parent).await {
            let message = shortcuts::describe_error(&error);
            tracing::warn!(%error, "global shortcut registration failed");
            let _ = error_sender.send(ShortcutEvent::Failed(message)).await;
        }
    });
}

fn handle_shortcut_event(state: &Rc<AppState>, event: ShortcutEvent) {
    match event {
        ShortcutEvent::Registered(descriptions) => {
            state.shortcut_registered.set(true);
            tracing::info!(?descriptions, "global shortcuts registered");
            let message = if descriptions.is_empty() {
                "Global shortcuts registered, but the portal returned no trigger description."
                    .to_string()
            } else {
                format!(
                    "Global shortcuts registered: {}. They stay active while Snipunix is running.",
                    descriptions.join("; ")
                )
            };
            state.set_status(&message);
            state.toast(&message);
        }
        ShortcutEvent::Failed(message) => {
            state.set_status("Global shortcuts are not registered.");
            state.toast(&message);
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
