//! GTK4 + Libadwaita application shell and capture flow.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use snipunix_capture::{
    CaptureBackend, CaptureError, CaptureOptions, CapturedImage, PortalScreenshot,
};
use snipunix_core::CaptureMapping;
use snipunix_overlay::{OverlayResult, collect_monitors, layout_from_monitors, present};

use crate::cli::{CaptureTarget, Cli};
use crate::shortcuts::{self, ShortcutEvent};

pub const APP_ID: &str = "io.github.alokverma9.snipunix";

pub fn run(cli: Cli) -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let initial = Rc::new(RefCell::new(Some(cli)));

    app.connect_activate(move |app| {
        if app.active_window().is_some() {
            return;
        }
        let cli = initial.borrow_mut().take().unwrap_or_default();
        activate(app, cli);
    });

    app.set_accels_for_action("app.capture-region", &["<Control><Shift>s"]);
    app.set_accels_for_action("app.capture-screen", &["<Control><Shift>a"]);
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
    pending_capture: RefCell<Option<CapturedImage>>,
    exit_after: Cell<bool>,
    launch_target: Cell<Option<CaptureTarget>>,
    register_shortcuts_on_start: Cell<bool>,
    shortcut_registered: Cell<bool>,
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
    content.append(&action_bar);

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

    Rc::new(AppState {
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
        pending_capture: RefCell::new(None),
        exit_after: Cell::new(cli.exit_after),
        launch_target: Cell::new(cli.target),
        register_shortcuts_on_start: Cell::new(cli.register_shortcuts),
        shortcut_registered: Cell::new(false),
    })
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
    state.set_busy(true);
    state.set_status("Requesting a screenshot from the desktop portal…");

    let window = state.window.clone();
    let state = Rc::clone(state);
    glib::spawn_future_local(async move {
        let parent = if window.is_realized() {
            ashpd::WindowIdentifier::from_native(&window).await
        } else {
            None
        };
        let backend = PortalScreenshot::default();
        let result = backend
            .capture(CaptureOptions {
                interactive: false,
                parent,
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
    match target {
        CaptureTarget::Screen => {
            state.display(&image);
            state.set_status(format!(
                "Captured the full desktop ({} × {}).",
                image.width(),
                image.height()
            ));
            save_pending(state, &image);
            state.quit_if_requested();
        }
        CaptureTarget::Region => {
            state.pending_capture.replace(Some(image));
            begin_overlay(state);
        }
    }
}

fn handle_capture_error(state: &Rc<AppState>, error: &CaptureError) {
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

fn begin_overlay(state: &Rc<AppState>) {
    let Some(full) = state.pending_capture.borrow().clone() else {
        state.toast("No captured image is available.");
        return;
    };
    let monitors = match collect_monitors() {
        Ok(monitors) => monitors,
        Err(error) => {
            state.toast(format!("Could not read the monitor layout: {error}"));
            return;
        }
    };
    let layout = match layout_from_monitors(&monitors) {
        Ok(layout) => layout,
        Err(error) => {
            state.toast(format!("Could not build the monitor layout: {error}"));
            return;
        }
    };
    let mapping = match CaptureMapping::new(layout.bounding_box(), full.size()) {
        Ok(mapping) => mapping,
        Err(error) => {
            state.toast(format!("Could not map the captured image: {error}"));
            return;
        }
    };

    let texture = texture_from_image(&full);
    state.overlay_active.set(true);
    state.set_status("Drag to select a region; release captures it. Escape cancels.");

    let state_done = Rc::clone(state);
    let presented = present(&state.app, texture, mapping, monitors, move |result| {
        state_done.overlay_active.set(false);
        let full = state_done.pending_capture.borrow_mut().take();
        match result {
            OverlayResult::Selected { image, .. } => {
                let Some(full) = full else {
                    state_done.toast("The captured image is no longer available.");
                    return;
                };
                tracing::debug!(?image, "cropping selected region");
                match full.crop(image) {
                    Ok(cropped) => {
                        tracing::debug!(
                            width = cropped.width(),
                            height = cropped.height(),
                            "cropped selected region"
                        );
                        state_done.display(&cropped);
                        state_done.set_status(format!(
                            "Selected region ({} × {}).",
                            cropped.width(),
                            cropped.height()
                        ));
                        save_pending(&state_done, &cropped);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not crop the region");
                        state_done.toast(format!("Could not crop the region: {error}"));
                    }
                }
            }
            OverlayResult::Cancelled => {
                state_done.set_status("Selection cancelled.");
            }
        }
        state_done.quit_if_requested();
    });

    if let Err(error) = presented {
        state.overlay_active.set(false);
        state.toast(format!("Could not open the selection overlay: {error}"));
    }
}

fn save_pending(state: &AppState, image: &CapturedImage) -> bool {
    let Some(path) = state.pending_output.borrow_mut().take() else {
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
            state.pending_output.replace(Some(path));
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
