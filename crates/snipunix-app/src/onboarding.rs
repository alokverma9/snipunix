//! First-run onboarding.
//!
//! The dialog primes the screenshot permission prompt, offers to register the
//! global shortcuts, and lets the user pick a save folder. It is intentionally
//! dismissible: everything it configures also lives in Settings.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use snipunix_settings::Settings;

use crate::settings_ui;

/// Shows the welcome dialog. `register_shortcuts` is only called when the user
/// keeps the shortcut option enabled and finishes onboarding.
pub fn show(
    settings: Rc<RefCell<Settings>>,
    path: PathBuf,
    parent: &gtk::Window,
    register_shortcuts: Rc<dyn Fn()>,
    open_keyboard_settings: Rc<dyn Fn()>,
) {
    let window = adw::Window::builder()
        .title("Welcome to Snipunix")
        .transient_for(parent)
        .modal(true)
        .default_width(620)
        .default_height(780)
        .build();

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let hero = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let icon = gtk::Image::from_icon_name("camera-photo-symbolic");
    icon.set_pixel_size(72);
    icon.set_halign(gtk::Align::Center);
    let title = gtk::Label::new(None);
    title.set_markup("<span size='xx-large' weight='bold'>Welcome to Snipunix</span>");
    title.set_halign(gtk::Align::Center);
    let subtitle = gtk::Label::new(Some(
        "Capture a region or the full screen, annotate it, and copy or save it. \
         Everything happens on this device.",
    ));
    subtitle.set_wrap(true);
    subtitle.set_justify(gtk::Justification::Center);
    subtitle.add_css_class("dim-label");
    subtitle.set_halign(gtk::Align::Center);
    hero.append(&icon);
    hero.append(&title);
    hero.append(&subtitle);
    content.append(&hero);

    // 1. Screenshot permission priming.
    let permission_group = adw::PreferencesGroup::builder()
        .title("Screenshots")
        .description(
            "Snipunix uses the desktop Screenshot portal. Your desktop asks for \
             permission the first time; captures are never taken in the background.",
        )
        .build();
    let permission = adw::ActionRow::builder()
        .title("Screenshot permission")
        .subtitle("Check that the screenshot portal is available")
        .build();
    let check = gtk::Button::with_label("Check");
    let permission_row = permission.clone();
    check.connect_clicked(move |button| {
        button.set_sensitive(false);
        permission_row.set_subtitle("Checking the desktop portal…");
        let row = permission_row.clone();
        let button = button.clone();
        glib::spawn_future_local(async move {
            let message = match snipunix_capture::probe_portal().await {
                Ok(info) => format!(
                    "Screenshot portal version {} is available. \
                     Your desktop will show a permission prompt on the first capture.",
                    info.version
                ),
                Err(error) => format!(
                    "The screenshot portal is unavailable: {error}. \
                     Install xdg-desktop-portal and a desktop backend."
                ),
            };
            row.set_subtitle(&message);
            button.set_sensitive(true);
        });
    });
    permission.add_suffix(&check);
    permission.set_activatable_widget(Some(&check));
    permission_group.add(&permission);
    content.append(&permission_group);

    // 2. Global shortcuts.
    let current = settings.borrow().clone();
    let shortcut_group = adw::PreferencesGroup::builder()
        .title("Global shortcuts")
        .description(
            "Snipunix can ask the desktop for global shortcuts. The portal may \
             remap them; the accepted triggers appear in Settings.",
        )
        .build();
    let register = adw::SwitchRow::builder()
        .title("Register global shortcuts")
        .subtitle(format!(
            "Region: {} · Screen: {}",
            current.shortcuts.region_trigger, current.shortcuts.screen_trigger
        ))
        .active(true)
        .build();
    shortcut_group.add(&register);

    let manual = adw::ActionRow::builder()
        .title("Prefer a manual shortcut?")
        .subtitle("Open your desktop's keyboard settings and bind `snipunix --capture region`")
        .build();
    let open = gtk::Button::with_label("Open");
    let callback = Rc::clone(&open_keyboard_settings);
    open.connect_clicked(move |_| callback());
    manual.add_suffix(&open);
    manual.set_activatable_widget(Some(&open));
    shortcut_group.add(&manual);
    content.append(&shortcut_group);

    // 3. Save folder.
    let folder_group = adw::PreferencesGroup::builder()
        .title("Saving")
        .description("Choose where screenshots go when you save or auto-save.")
        .build();
    let folder = adw::ActionRow::builder()
        .title("Save folder")
        .subtitle(current.files.directory_or_default().display().to_string())
        .build();
    folder.set_subtitle_lines(2);
    let choose = gtk::Button::with_label("Choose…");
    let folder_row = folder.clone();
    let settings_for_folder = Rc::clone(&settings);
    let path_for_folder = path.clone();
    let window_for_choose = window.clone();
    choose.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder()
            .title("Choose save folder")
            .modal(true)
            .build();
        if let Some(directory) = settings_for_folder.borrow().files.directory.clone() {
            dialog.set_initial_folder(Some(&gio::File::for_path(directory)));
        }
        let parent = window_for_choose.clone();
        let row = folder_row.clone();
        let settings = Rc::clone(&settings_for_folder);
        let path = path_for_folder.clone();
        dialog.select_folder(Some(&parent), None::<&gio::Cancellable>, move |result| {
            let Ok(file) = result else {
                return;
            };
            let Some(directory) = file.path() else {
                return;
            };
            row.set_subtitle(&directory.display().to_string());
            {
                let mut settings = settings.borrow_mut();
                settings.files.directory = Some(directory);
                settings.sanitize();
            }
            settings_ui::persist(&settings.borrow(), &path);
        });
    });
    folder.add_suffix(&choose);
    folder.set_activatable_widget(Some(&choose));
    folder_group.add(&folder);

    let privacy = adw::ActionRow::builder()
        .title("Private by design")
        .subtitle(
            "Local OCR only. No telemetry, no cloud upload, and no model download \
             unless you explicitly ask for one.",
        )
        .build();
    privacy.set_subtitle_lines(3);
    folder_group.add(&privacy);
    content.append(&folder_group);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let skip = gtk::Button::with_label("Skip");
    let start = gtk::Button::with_label("Get Started");
    start.add_css_class("suggested-action");
    actions.append(&skip);
    actions.append(&start);
    content.append(&actions);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();
    toolbar.set_content(Some(&scroller));
    window.set_content(Some(&toolbar));

    let finished = Rc::new(Cell::new(false));
    let complete: Rc<dyn Fn(bool)> = {
        let settings = Rc::clone(&settings);
        let path = path.clone();
        let register_shortcuts = Rc::clone(&register_shortcuts);
        let finished = Rc::clone(&finished);
        Rc::new(move |wants_shortcuts: bool| {
            if finished.replace(true) {
                return;
            }
            {
                let mut settings = settings.borrow_mut();
                settings.onboarding_complete = true;
                settings.shortcuts.register_on_startup = wants_shortcuts;
                settings.sanitize();
            }
            settings_ui::persist(&settings.borrow(), &path);
            if wants_shortcuts {
                register_shortcuts();
            }
        })
    };

    let skip_complete = Rc::clone(&complete);
    let skip_window = window.clone();
    skip.connect_clicked(move |_| {
        skip_complete(false);
        skip_window.close();
    });

    let start_complete = Rc::clone(&complete);
    let start_window = window.clone();
    start.connect_clicked(move |_| {
        let wants_shortcuts = register.is_active();
        start_complete(wants_shortcuts);
        start_window.close();
    });

    // Closing the welcome window also counts as completing onboarding; the
    // flag only prevents the dialog from appearing on every launch.
    let close_complete = Rc::clone(&complete);
    window.connect_close_request(move |_| {
        close_complete(false);
        glib::Propagation::Proceed
    });

    window.present();
}
