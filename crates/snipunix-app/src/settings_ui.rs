//! Libadwaita settings pages.
//!
//! Every control writes through to `snipunix-settings` immediately. The crate
//! clamps invalid values before the atomic save, so closing the dialog never
//! leaves a half-applied state behind.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use snipunix_i18n::{tr, tr_fmt};
use snipunix_settings::{CaptureMode, SaveFormat, Settings};

use crate::app::APP_ID;

/// Callbacks the settings dialog needs from the application shell.
#[derive(Clone)]
pub struct Callbacks {
    /// Called after a setting has been persisted.
    pub on_saved: Rc<dyn Fn()>,
    /// Re-registers global shortcuts with the current preferred triggers.
    pub register_shortcuts: Rc<dyn Fn()>,
    /// Opens the desktop's keyboard-shortcut panel.
    pub open_keyboard_settings: Rc<dyn Fn()>,
}

/// Shows the preferences dialog for `settings`.
pub fn present(
    settings: Rc<RefCell<Settings>>,
    path: PathBuf,
    parent: &gtk::Window,
    callbacks: Callbacks,
) {
    let dialog = adw::PreferencesDialog::builder()
        .title(tr!("Snipunix Settings"))
        .search_enabled(true)
        .build();

    let context = Context {
        settings,
        path,
        callbacks,
        window: parent.clone(),
    };

    dialog.add(&capture_page(&context));
    dialog.add(&shortcut_page(&context));
    dialog.add(&files_page(&context));
    dialog.add(&recording_page(&context));
    dialog.add(&ocr_page(&context));
    dialog.add(&appearance_page(&context));
    dialog.add(&about_page(&context));
    dialog.present(Some(parent));
}

struct Context {
    settings: Rc<RefCell<Settings>>,
    path: PathBuf,
    callbacks: Callbacks,
    window: gtk::Window,
}

fn capture_page(context: &Context) -> adw::PreferencesPage {
    let settings = context.settings.borrow().clone();
    let page = adw::PreferencesPage::builder()
        .title(tr!("Capture"))
        .icon_name("camera-photo-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(tr!("Capture defaults"))
        .description(tr!(
            "These apply to the Capture Region and Capture Screen actions."
        ))
        .build();

    let mode = adw::ComboRow::builder()
        .title(tr!("Default mode"))
        .subtitle(tr!("What the tray and global shortcut open"))
        .model(&gtk::StringList::new(&[
            &tr!("Select a region"),
            &tr!("Full screen"),
        ]))
        .selected(match settings.capture.default_mode {
            CaptureMode::Region => 0,
            CaptureMode::Screen => 1,
        })
        .build();
    let context_mode = context.clone_handle();
    mode.connect_selected_notify(move |row| {
        context_mode.update(|settings| {
            settings.capture.default_mode = if row.selected() == 1 {
                CaptureMode::Screen
            } else {
                CaptureMode::Region
            };
        });
    });
    group.add(&mode);

    let delay = adw::SpinRow::builder()
        .title(tr!("Delay"))
        .subtitle(tr!("Seconds to wait before the screen is frozen"))
        .adjustment(&gtk::Adjustment::new(
            f64::from(settings.capture.delay),
            0.0,
            f64::from(snipunix_settings::MAX_DELAY_SECONDS),
            1.0,
            1.0,
            0.0,
        ))
        .digits(0)
        .snap_to_ticks(true)
        .build();
    let context_delay = context.clone_handle();
    delay.connect_value_notify(move |row| {
        let value = row
            .value()
            .clamp(0.0, f64::from(snipunix_settings::MAX_DELAY_SECONDS));
        context_delay.update(|settings| settings.capture.delay = value as u32);
    });
    group.add(&delay);

    let copy = adw::SwitchRow::builder()
        .title(tr!("Copy after capture"))
        .subtitle(tr!("Put the finished screenshot on the clipboard"))
        .active(settings.capture.copy_after_capture)
        .build();
    let context_copy = context.clone_handle();
    copy.connect_active_notify(move |row| {
        context_copy.update(|settings| settings.capture.copy_after_capture = row.is_active());
    });
    group.add(&copy);

    let remember = adw::SwitchRow::builder()
        .title(tr!("Remember last region"))
        .subtitle(tr!(
            "Pre-fill the selection overlay with the previous rectangle"
        ))
        .active(settings.capture.remember_last_region)
        .build();
    let context_remember = context.clone_handle();
    remember.connect_active_notify(move |row| {
        context_remember.update(|settings| {
            settings.capture.remember_last_region = row.is_active();
        });
    });
    group.add(&remember);

    let cursor = adw::SwitchRow::builder()
        .title(tr!("Include pointer"))
        .subtitle(tr!("Draw the mouse pointer into native X11 captures"))
        .active(settings.capture.include_cursor)
        .build();
    let context_cursor = context.clone_handle();
    cursor.connect_active_notify(move |row| {
        context_cursor.update(|settings| settings.capture.include_cursor = row.is_active());
    });
    group.add(&cursor);

    page.add(&group);
    page
}

fn shortcut_page(context: &Context) -> adw::PreferencesPage {
    let settings = context.settings.borrow().clone();
    let page = adw::PreferencesPage::builder()
        .title(tr!("Shortcuts"))
        .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(tr!("Global shortcuts"))
        .description(tr!(
            "Registration goes through the desktop's GlobalShortcuts portal. \
             The portal may remap a trigger; the triggers shown below are the \
             ones it actually accepted."
        ))
        .build();

    let register = adw::SwitchRow::builder()
        .title(tr!("Register on startup"))
        .subtitle(tr!(
            "Ask the portal for global shortcuts whenever Snipunix starts"
        ))
        .active(settings.shortcuts.register_on_startup)
        .build();
    let context_register = context.clone_handle();
    register.connect_active_notify(move |row| {
        context_register.update(|settings| {
            settings.shortcuts.register_on_startup = row.is_active();
        });
    });
    group.add(&register);

    let region = adw::EntryRow::builder()
        .title(tr!("Region trigger"))
        .text(settings.shortcuts.region_trigger.clone())
        .build();
    region.set_tooltip_text(Some(&tr!(
        "Portal trigger syntax, for example CTRL+SHIFT+s. Press Enter to apply."
    )));
    let context_region = context.clone_handle();
    region.connect_apply(move |row| {
        let value = row.text().trim().to_string();
        if value.is_empty() {
            row.set_text("CTRL+SHIFT+s");
            return;
        }
        context_region.update(|settings| settings.shortcuts.region_trigger = value);
    });
    group.add(&region);

    let screen = adw::EntryRow::builder()
        .title(tr!("Screen trigger"))
        .text(settings.shortcuts.screen_trigger.clone())
        .build();
    screen.set_tooltip_text(Some(&tr!(
        "Portal trigger syntax, for example CTRL+SHIFT+a. Press Enter to apply."
    )));
    let context_screen = context.clone_handle();
    screen.connect_apply(move |row| {
        let value = row.text().trim().to_string();
        if value.is_empty() {
            row.set_text("CTRL+SHIFT+a");
            return;
        }
        context_screen.update(|settings| settings.shortcuts.screen_trigger = value);
    });
    group.add(&screen);

    let bound = adw::ActionRow::builder()
        .title(tr!("Registered triggers"))
        .subtitle(if settings.shortcuts.bound_description.is_empty() {
            tr!("Not registered yet")
        } else {
            settings.shortcuts.bound_description.clone()
        })
        .subtitle_selectable(true)
        .build();
    bound.set_subtitle_lines(3);
    let register_button = gtk::Button::with_label(&tr!("Register Now"));
    register_button.add_css_class("suggested-action");
    let callback = Rc::clone(&context.callbacks.register_shortcuts);
    register_button.connect_clicked(move |_| callback());
    bound.add_suffix(&register_button);
    group.add(&bound);

    page.add(&group);

    let help = adw::PreferencesGroup::builder()
        .title(tr!("Troubleshooting"))
        .description(tr!(
            "If the portal refuses the request, bind a custom shortcut in your \
             desktop's keyboard settings to `snipunix --capture region` and \
             `snipunix --capture screen`."
        ))
        .build();
    let row = adw::ActionRow::builder()
        .title(tr!("Desktop keyboard settings"))
        .subtitle(tr!("Open the system panel to bind a manual shortcut"))
        .build();
    let open = gtk::Button::with_label(&tr!("Open"));
    let callback = Rc::clone(&context.callbacks.open_keyboard_settings);
    open.connect_clicked(move |_| callback());
    row.add_suffix(&open);
    row.set_activatable_widget(Some(&open));
    help.add(&row);
    page.add(&help);
    page
}

fn files_page(context: &Context) -> adw::PreferencesPage {
    let settings = context.settings.borrow().clone();
    let page = adw::PreferencesPage::builder()
        .title(tr!("Files"))
        .icon_name("folder-pictures-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(tr!("Saving"))
        .description(tr!(
            "Snipunix never writes a file without a save action or auto-save enabled."
        ))
        .build();

    let auto_save = adw::SwitchRow::builder()
        .title(tr!("Auto-save after capture"))
        .subtitle(tr!("Write a file as soon as a capture completes"))
        .active(settings.files.auto_save)
        .build();
    let context_auto_save = context.clone_handle();
    auto_save.connect_active_notify(move |row| {
        context_auto_save.update(|settings| settings.files.auto_save = row.is_active());
    });
    group.add(&auto_save);

    let directory = adw::ActionRow::builder()
        .title(tr!("Save folder"))
        .subtitle(settings.files.directory_or_default().display().to_string())
        .build();
    let choose = gtk::Button::with_label(&tr!("Choose…"));
    let context_choose = context.clone_handle();
    let directory_row = directory.clone();
    choose.connect_clicked(move |_| {
        choose_folder(
            &context_choose.window,
            Some(
                context_choose
                    .settings
                    .borrow()
                    .files
                    .directory_or_default(),
            ),
            {
                let context = context_choose.clone_handle();
                let directory_row = directory_row.clone();
                move |path| {
                    directory_row.set_subtitle(&path.display().to_string());
                    context.update(|settings| settings.files.directory = Some(path.clone()));
                }
            },
        );
    });
    directory.add_suffix(&choose);
    directory.set_activatable_widget(Some(&choose));
    group.add(&directory);

    let template = adw::EntryRow::builder()
        .title(tr!("Filename template"))
        .text(settings.files.filename_template.clone())
        .build();
    template.set_tooltip_text(Some(&tr!(
        "Expanded with the local capture time; for example Screenshot_%Y-%m-%d_%H-%M-%S. \
         Press Enter to apply."
    )));
    let context_template = context.clone_handle();
    template.connect_apply(move |row| {
        let value = row.text().trim().to_string();
        if value.is_empty() {
            row.set_text(snipunix_settings::DEFAULT_FILENAME_TEMPLATE);
            return;
        }
        context_template.update(|settings| settings.files.filename_template = value);
    });
    group.add(&template);

    let format = adw::ComboRow::builder()
        .title(tr!("Format"))
        .subtitle(tr!("Default export format"))
        .model(&gtk::StringList::new(&[
            SaveFormat::Png.label(),
            SaveFormat::Jpeg.label(),
            SaveFormat::Webp.label(),
        ]))
        .selected(match settings.files.format {
            SaveFormat::Png => 0,
            SaveFormat::Jpeg => 1,
            SaveFormat::Webp => 2,
        })
        .build();
    let context_format = context.clone_handle();
    format.connect_selected_notify(move |row| {
        context_format.update(|settings| {
            settings.files.format = match row.selected() {
                1 => SaveFormat::Jpeg,
                2 => SaveFormat::Webp,
                _ => SaveFormat::Png,
            };
        });
    });
    group.add(&format);

    let quality = adw::SpinRow::builder()
        .title(tr!("JPEG quality"))
        .subtitle(tr!("Only used when the format is JPEG"))
        .adjustment(&gtk::Adjustment::new(
            f64::from(settings.files.jpeg_quality),
            1.0,
            100.0,
            1.0,
            10.0,
            0.0,
        ))
        .digits(0)
        .snap_to_ticks(true)
        .build();
    let context_quality = context.clone_handle();
    quality.connect_value_notify(move |row| {
        let value = row.value().clamp(1.0, 100.0);
        context_quality.update(|settings| settings.files.jpeg_quality = value as u8);
    });
    group.add(&quality);

    page.add(&group);
    page
}

fn recording_page(context: &Context) -> adw::PreferencesPage {
    let settings = context.settings.borrow().clone();
    let page = adw::PreferencesPage::builder()
        .title(tr!("Recording"))
        .icon_name("media-record-symbolic")
        .build();

    let output_group = adw::PreferencesGroup::builder()
        .title(tr!("Output"))
        .description(tr!("Recordings are written as H.264 MP4 files."))
        .build();
    let directory = adw::ActionRow::builder()
        .title(tr!("Recording folder"))
        .subtitle(settings.recording_directory().display().to_string())
        .build();
    let choose = gtk::Button::with_label(&tr!("Choose…"));
    let context_choose = context.clone_handle();
    let directory_row = directory.clone();
    choose.connect_clicked(move |_| {
        choose_folder(
            &context_choose.window,
            Some(context_choose.settings.borrow().recording_directory()),
            {
                let context = context_choose.clone_handle();
                let directory_row = directory_row.clone();
                move |path| {
                    directory_row.set_subtitle(&path.display().to_string());
                    context.update(|settings| settings.recording.directory = Some(path.clone()));
                }
            },
        );
    });
    directory.add_suffix(&choose);
    directory.set_activatable_widget(Some(&choose));
    output_group.add(&directory);
    page.add(&output_group);

    let video_group = adw::PreferencesGroup::builder()
        .title(tr!("Video"))
        .description(tr!("Frame rate and bitrate apply to the next recording."))
        .build();
    let fps = adw::SpinRow::builder()
        .title(tr!("Frame rate"))
        .subtitle(tr!("Frames per second (10–60)"))
        .adjustment(&gtk::Adjustment::new(
            f64::from(settings.recording.fps),
            f64::from(snipunix_record::MIN_FPS),
            f64::from(snipunix_record::MAX_FPS),
            1.0,
            5.0,
            0.0,
        ))
        .digits(0)
        .snap_to_ticks(true)
        .build();
    let context_fps = context.clone_handle();
    fps.connect_value_notify(move |row| {
        let value = row.value().round() as u32;
        context_fps.update(|settings| settings.recording.fps = value);
    });
    video_group.add(&fps);

    let bitrate = adw::SpinRow::builder()
        .title(tr!("Bitrate"))
        .subtitle(tr!("Kilobits per second (500–50000)"))
        .adjustment(&gtk::Adjustment::new(
            f64::from(settings.recording.bitrate_kbps),
            f64::from(snipunix_record::MIN_BITRATE_KBPS),
            f64::from(snipunix_record::MAX_BITRATE_KBPS),
            500.0,
            2000.0,
            0.0,
        ))
        .digits(0)
        .snap_to_ticks(true)
        .build();
    let context_bitrate = context.clone_handle();
    bitrate.connect_value_notify(move |row| {
        let value = row.value().round() as u32;
        context_bitrate.update(|settings| settings.recording.bitrate_kbps = value);
    });
    video_group.add(&bitrate);

    let cursor = adw::SwitchRow::builder()
        .title(tr!("Include the pointer"))
        .subtitle(tr!(
            "Ask the compositor to draw the cursor into the recording"
        ))
        .active(settings.recording.include_cursor)
        .build();
    let context_cursor = context.clone_handle();
    cursor.connect_active_notify(move |row| {
        context_cursor.update(|settings| settings.recording.include_cursor = row.is_active());
    });
    video_group.add(&cursor);
    page.add(&video_group);

    let audio_group = adw::PreferencesGroup::builder()
        .title(tr!("Audio"))
        .description(tr!(
            "Both sources are off by default. Each one is recorded separately and \
             mixed into the MP4."
        ))
        .build();
    let microphone = adw::SwitchRow::builder()
        .title(tr!("Microphone"))
        .subtitle(tr!("Record the default microphone"))
        .active(settings.recording.microphone)
        .build();
    let context_microphone = context.clone_handle();
    microphone.connect_active_notify(move |row| {
        context_microphone.update(|settings| settings.recording.microphone = row.is_active());
    });
    audio_group.add(&microphone);

    let system = adw::SwitchRow::builder()
        .title(tr!("System audio"))
        .subtitle(tr!("Record the default output monitor (what you hear)"))
        .active(settings.recording.system_audio)
        .build();
    let context_system = context.clone_handle();
    system.connect_active_notify(move |row| {
        context_system.update(|settings| settings.recording.system_audio = row.is_active());
    });
    audio_group.add(&system);
    page.add(&audio_group);

    let permission_group = adw::PreferencesGroup::builder()
        .title(tr!("ScreenCast permission"))
        .description(tr!(
            "Snipunix remembers the source you last allowed. Reset it to see the \
             desktop picker again on the next recording."
        ))
        .build();
    let reset = adw::ActionRow::builder()
        .title(tr!("Forget remembered source"))
        .subtitle(if settings.recording.restore_token.is_some() {
            tr!("A source is currently remembered")
        } else {
            tr!("No source is remembered")
        })
        .build();
    let reset_button = gtk::Button::with_label(&tr!("Reset"));
    reset_button.set_valign(gtk::Align::Center);
    let context_reset = context.clone_handle();
    let reset_row = reset.clone();
    reset_button.connect_clicked(move |_| {
        reset_row.set_subtitle(&tr!("No source is remembered"));
        context_reset.update(|settings| settings.recording.restore_token = None);
    });
    reset.add_suffix(&reset_button);
    reset.set_activatable_widget(Some(&reset_button));
    permission_group.add(&reset);
    page.add(&permission_group);

    page
}

fn ocr_page(context: &Context) -> adw::PreferencesPage {
    let settings = context.settings.borrow().clone();
    let page = adw::PreferencesPage::builder()
        .title(tr!("Text Recognition"))
        .icon_name("edit-find-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(tr!("Local OCR"))
        .description(tr!(
            "Recognition runs on this device with a local model. No screenshot \
             or recognised text is uploaded."
        ))
        .build();

    let auto_copy = adw::SwitchRow::builder()
        .title(tr!("Auto-copy extracted text"))
        .subtitle(tr!(
            "Place the recognised text on the clipboard when it finishes"
        ))
        .active(settings.ocr.auto_copy)
        .build();
    let context_auto_copy = context.clone_handle();
    auto_copy.connect_active_notify(move |row| {
        context_auto_copy.update(|settings| settings.ocr.auto_copy = row.is_active());
    });
    group.add(&auto_copy);

    let line_breaks = adw::SwitchRow::builder()
        .title(tr!("Remove line breaks"))
        .subtitle(tr!("Join recognised lines into flowing text"))
        .active(settings.ocr.remove_line_breaks)
        .build();
    let context_line_breaks = context.clone_handle();
    line_breaks.connect_active_notify(move |row| {
        context_line_breaks.update(|settings| settings.ocr.remove_line_breaks = row.is_active());
    });
    group.add(&line_breaks);

    let model_dir = settings.model_directory();
    let model = adw::ActionRow::builder()
        .title(tr!("Model folder"))
        .subtitle(model_dir.display().to_string())
        .build();
    model.set_subtitle_lines(2);
    let model_row = model.clone();
    let choose = gtk::Button::with_label(&tr!("Choose…"));
    let context_model = context.clone_handle();
    choose.connect_clicked(move |_| {
        choose_folder(
            &context_model.window,
            Some(context_model.settings.borrow().model_directory()),
            {
                let context = context_model.clone_handle();
                let model_row = model_row.clone();
                move |path| {
                    model_row.set_subtitle(&path.display().to_string());
                    context.update(|settings| settings.ocr.model_directory = Some(path.clone()));
                }
            },
        );
    });
    model.add_suffix(&choose);
    group.add(&model);

    let download = adw::ActionRow::builder()
        .title(tr!("Download OCR models"))
        .subtitle(tr!(
            "Explicit opt-in. Two verified files (~12 MB) are written to the model folder."
        ))
        .build();
    let download_button = gtk::Button::with_label(&tr!("Download"));
    let download_row = download.clone();
    let context_download = context.clone_handle();
    download_button.connect_clicked(move |button| {
        let directory = context_download.settings.borrow().model_directory();
        button.set_sensitive(false);
        download_row.set_subtitle(&tr!("Downloading and verifying models…"));
        let button = button.clone();
        let download_row = download_row.clone();
        glib::spawn_future_local(async move {
            let (sender, receiver) = async_channel::bounded(1);
            let directory_for_thread = directory.clone();
            std::thread::spawn(move || {
                let result = snipunix_ocr::download_models(&directory_for_thread)
                    .map(|_| ())
                    .map_err(|error| error.to_string());
                let _ = sender.send_blocking(result);
            });
            match receiver.recv().await {
                Ok(Ok(())) => {
                    download_row
                        .set_subtitle(&tr_fmt!("Models installed in {0}", directory.display()));
                }
                Ok(Err(error)) => {
                    download_row.set_subtitle(&tr_fmt!("Download failed: {0}", error));
                }
                Err(_) => {
                    download_row.set_subtitle(&tr!("The download stopped unexpectedly."));
                }
            }
            button.set_sensitive(true);
        });
    });
    download.add_suffix(&download_button);
    group.add(&download);

    page.add(&group);
    page
}

fn appearance_page(context: &Context) -> adw::PreferencesPage {
    let settings = context.settings.borrow().clone();
    let page = adw::PreferencesPage::builder()
        .title(tr!("Appearance"))
        .icon_name("applications-graphics-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(tr!("Selection overlay"))
        .description(tr!("Presentation options for the capture overlay."))
        .build();

    let magnifier = adw::SwitchRow::builder()
        .title(tr!("Magnifier"))
        .subtitle(tr!("Show a magnified pixel grid while selecting"))
        .active(settings.appearance.show_magnifier)
        .build();
    let context_magnifier = context.clone_handle();
    magnifier.connect_active_notify(move |row| {
        context_magnifier.update(|settings| settings.appearance.show_magnifier = row.is_active());
    });
    group.add(&magnifier);

    let dim = adw::SpinRow::builder()
        .title(tr!("Dim strength"))
        .subtitle(tr!("How dark the area outside the selection becomes"))
        .adjustment(&gtk::Adjustment::new(
            settings.appearance.dim_strength,
            0.10,
            0.85,
            0.05,
            0.10,
            0.0,
        ))
        .digits(2)
        .snap_to_ticks(true)
        .build();
    let context_dim = context.clone_handle();
    dim.connect_value_notify(move |row| {
        let value = row.value().clamp(0.10, 0.85);
        context_dim.update(|settings| settings.appearance.dim_strength = value);
    });
    group.add(&dim);

    let motion = adw::SwitchRow::builder()
        .title(tr!("Reduced motion"))
        .subtitle(tr!("Disable overlay animations"))
        .active(settings.appearance.reduced_motion)
        .build();
    let context_motion = context.clone_handle();
    motion.connect_active_notify(move |row| {
        context_motion.update(|settings| settings.appearance.reduced_motion = row.is_active());
    });
    group.add(&motion);

    page.add(&group);
    page
}

fn about_page(context: &Context) -> adw::PreferencesPage {
    let settings = context.settings.borrow().clone();
    let page = adw::PreferencesPage::builder()
        .title(tr!("About"))
        .icon_name("help-about-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(tr!("Snipunix"))
        .description(tr!(
            "Screenshot. Annotate. Share. All processing stays on this device."
        ))
        .build();

    let version = adw::ActionRow::builder()
        .title(tr!("Version"))
        .subtitle(env!("CARGO_PKG_VERSION"))
        .build();
    group.add(&version);

    let id = adw::ActionRow::builder()
        .title(tr!("Application ID"))
        .subtitle(APP_ID)
        .subtitle_selectable(true)
        .build();
    group.add(&id);

    let license = adw::ActionRow::builder()
        .title(tr!("License"))
        .subtitle(tr!("GNU General Public License v3.0"))
        .build();
    group.add(&license);

    let privacy = adw::ActionRow::builder()
        .title(tr!("Privacy"))
        .subtitle(tr!(
            "No telemetry. No cloud OCR. Downloads happen only when requested."
        ))
        .build();
    group.add(&privacy);

    page.add(&group);

    let diagnostics = adw::PreferencesGroup::builder()
        .title(tr!("Diagnostics"))
        .description(tr!(
            "Snipunix never sends anything anywhere. These settings only control \
             what is written to this device."
        ))
        .build();

    let crash = adw::SwitchRow::builder()
        .title(tr!("Save crash reports"))
        .subtitle(tr!(
            "Off by default. When enabled, a crash report with the panic message \
             and a backtrace is written to the local state directory."
        ))
        .active(settings.diagnostics.crash_reports)
        .build();
    let context_crash = context.clone_handle();
    crash.connect_active_notify(move |row| {
        let enabled = row.is_active();
        context_crash.update(|settings| settings.diagnostics.crash_reports = enabled);
        snipunix_diagnostics::crash::set_enabled(enabled);
    });
    diagnostics.add(&crash);

    let keep = adw::SpinRow::builder()
        .title(tr!("Keep crash reports"))
        .subtitle(tr!("How many reports to keep on disk (1–100)"))
        .adjustment(&gtk::Adjustment::new(
            f64::from(settings.diagnostics.keep_crash_reports),
            1.0,
            100.0,
            1.0,
            10.0,
            0.0,
        ))
        .digits(0)
        .snap_to_ticks(true)
        .build();
    let context_keep = context.clone_handle();
    keep.connect_value_notify(move |row| {
        let value = row.value().clamp(1.0, 100.0) as u32;
        context_keep.update(|settings| settings.diagnostics.keep_crash_reports = value);
        snipunix_diagnostics::crash::set_keep(value as usize);
    });
    diagnostics.add(&keep);

    let export = adw::ActionRow::builder()
        .title(tr!("Export diagnostics"))
        .subtitle(tr!(
            "Write a report about this installation to a file you can attach to a \
             bug report. It contains no screen content and no recognised text."
        ))
        .build();
    export.set_subtitle_lines(3);
    let export_button = gtk::Button::with_label(&tr!("Export…"));
    export_button.add_css_class("suggested-action");
    let context_export = context.clone_handle();
    export_button.connect_clicked(move |_| {
        export_diagnostics(&context_export.window);
    });
    export.add_suffix(&export_button);
    export.set_activatable_widget(Some(&export_button));
    diagnostics.add(&export);

    page.add(&diagnostics);
    page
}

/// Asks for a file name and writes a diagnostics report to it.
fn export_diagnostics(parent: &gtk::Window) {
    let dialog = gtk::FileDialog::builder()
        .title(tr!("Export diagnostics"))
        .modal(true)
        .build();
    let suggested =
        snipunix_diagnostics::default_export_directory().join("snipunix-diagnostics.txt");
    if let Some(parent_dir) = suggested.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(parent_dir)));
    }
    let parent = parent.clone();
    dialog.save(Some(&parent), None::<&gio::Cancellable>, move |result| {
        let Ok(file) = result else {
            return;
        };
        let Some(path) = file.path() else {
            tracing::warn!("the diagnostics destination is not a local path");
            return;
        };
        let report = snipunix_diagnostics::collect(&snipunix_diagnostics::ReportOptions {
            backend: snipunix_capture::BackendKind::Auto,
            probe_x11: true,
            crash_directory: Some(snipunix_diagnostics::crash::directory()),
            include_translations: true,
        });
        match snipunix_diagnostics::write_report(&report, &path) {
            Ok(()) => tracing::info!(path = %path.display(), "wrote a diagnostics report"),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not write the report");
            }
        }
    });
}

/// Opens the desktop's privacy panel where screenshot permission is listed.
pub fn open_privacy_settings() -> Result<(), String> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktop = desktop.to_lowercase();
    let candidates: &[(&str, &[&str])] = if desktop.contains("kde") || desktop.contains("plasma") {
        &[("systemsettings", &["kcm_privacy"])]
    } else if desktop.contains("gnome") || desktop.is_empty() {
        &[("gnome-control-center", &["privacy"])]
    } else {
        &[
            ("gnome-control-center", &["privacy"]),
            ("systemsettings", &["kcm_privacy"]),
        ]
    };
    launch_first(candidates, "a privacy settings panel")
}

/// Explains how to allow screenshots after the portal denied permission.
pub fn show_permission_help(parent: &gtk::Window, detail: &str) {
    let dialog = adw::MessageDialog::builder()
        .heading(tr!("Screenshot permission was denied"))
        .body(tr_fmt!(
            "{0}\n\nOpen the desktop's privacy settings and allow Snipunix \
             (or the Screenshot portal) to take screenshots, then try again. \
             Snipunix never captures in the background.",
            detail
        ))
        .transient_for(parent)
        .modal(true)
        .build();
    dialog.add_response("close", &tr!("Close"));
    dialog.add_response("open", &tr!("Open Privacy Settings"));
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
    dialog.connect_response(None, move |dialog, response| {
        if response == "open" {
            if let Err(error) = open_privacy_settings() {
                tracing::warn!(%error, "could not open the privacy settings panel");
            }
        }
        dialog.close();
    });
    dialog.present();
}

/// Opens the desktop's keyboard shortcut panel.
pub fn open_keyboard_settings() -> Result<(), String> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktop = desktop.to_lowercase();
    let candidates: &[(&str, &[&str])] = if desktop.contains("gnome") {
        &[("gnome-control-center", &["keyboard"])]
    } else if desktop.contains("kde") || desktop.contains("plasma") {
        &[
            ("systemsettings", &["kcm_keys"]),
            ("systemsettings5", &["kcm_keys"]),
        ]
    } else if desktop.contains("xfce") {
        &[("xfce4-keyboard-settings", &[])]
    } else if desktop.contains("cinnamon") {
        &[("cinnamon-settings", &["keyboard"])]
    } else {
        &[
            ("gnome-control-center", &["keyboard"]),
            ("systemsettings", &["kcm_keys"]),
            ("xfce4-keyboard-settings", &[]),
        ]
    };

    launch_first(candidates, "a keyboard settings panel")
}

fn launch_first(candidates: &[(&str, &[&str])], description: &str) -> Result<(), String> {
    let mut errors = Vec::new();
    for (program, arguments) in candidates {
        match Command::new(program).args(*arguments).spawn() {
            Ok(_) => return Ok(()),
            Err(error) => errors.push(format!("{program}: {error}")),
        }
    }
    Err(format!(
        "could not start {description} ({})",
        errors.join("; ")
    ))
}

/// Explains how to bind a shortcut manually after the portal refuses.
pub fn show_shortcut_help(parent: &gtk::Window, detail: &str) {
    let dialog = adw::MessageDialog::builder()
        .heading(tr!("Global shortcuts are not registered"))
        .body(tr_fmt!(
            "{0}\n\nYou can bind a manual shortcut instead:\n\
             • GNOME: Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts\n\
             • KDE Plasma: System Settings → Shortcuts → Custom Shortcuts\n\
             • Then add `snipunix --capture region` and `snipunix --capture screen`.",
            detail
        ))
        .transient_for(parent)
        .modal(true)
        .build();
    dialog.add_response("close", &tr!("Close"));
    dialog.add_response("open", &tr!("Open Keyboard Settings"));
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
    dialog.connect_response(None, move |dialog, response| {
        if response == "open" {
            if let Err(error) = open_keyboard_settings() {
                tracing::warn!(%error, "could not open the keyboard settings panel");
            }
        }
        dialog.close();
    });
    dialog.present();
}

fn choose_folder(
    parent: &gtk::Window,
    current: Option<PathBuf>,
    on_selected: impl Fn(PathBuf) + 'static,
) {
    let dialog = gtk::FileDialog::builder()
        .title(tr!("Choose folder"))
        .modal(true)
        .build();
    if let Some(current) = current {
        if let Some(parent_dir) = current.parent() {
            dialog.set_initial_folder(Some(&gio::File::for_path(parent_dir)));
        }
    }
    let parent = parent.clone();
    dialog.select_folder(
        Some(&parent),
        None::<&gio::Cancellable>,
        move |result| match result {
            Ok(file) => match file.path() {
                Some(path) => on_selected(path),
                None => tracing::warn!("selected folder is not a local path"),
            },
            Err(error) => {
                if !error.matches(gtk::DialogError::Dismissed) {
                    tracing::warn!(%error, "could not choose a folder");
                }
            }
        },
    );
}

impl Context {
    fn clone_handle(&self) -> ContextHandle {
        ContextHandle {
            settings: Rc::clone(&self.settings),
            path: self.path.clone(),
            callbacks: self.callbacks.clone(),
            window: self.window.clone(),
        }
    }
}

struct ContextHandle {
    settings: Rc<RefCell<Settings>>,
    path: PathBuf,
    callbacks: Callbacks,
    window: gtk::Window,
}

impl ContextHandle {
    fn clone_handle(&self) -> ContextHandle {
        ContextHandle {
            settings: Rc::clone(&self.settings),
            path: self.path.clone(),
            callbacks: self.callbacks.clone(),
            window: self.window.clone(),
        }
    }

    fn update(&self, change: impl FnOnce(&mut Settings)) {
        let snapshot = {
            let mut settings = self.settings.borrow_mut();
            change(&mut settings);
            settings.sanitize();
            settings.clone()
        };
        persist(&snapshot, &self.path);
        (self.callbacks.on_saved)();
    }
}

pub(crate) fn persist(settings: &Settings, path: &Path) {
    if let Err(error) = settings.save(path) {
        tracing::warn!(%error, path = %path.display(), "could not save settings");
    }
}
