//! Best-effort StatusNotifierItem tray.
//!
//! GNOME only shows these items when the AppIndicator extension is enabled.
//! Spawn failures are returned to the caller, which logs them and continues:
//! every tray action also exists in the window and the CLI.

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{MenuItem, Tray};
use snipunix_i18n::tr;

/// Commands emitted by tray interactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    CaptureRegion,
    CaptureScreen,
    Show,
    Settings,
    Quit,
}

/// Owns the tray service for as long as the application runs.
pub struct TrayHandle(Handle<SnipunixTray>);

impl std::fmt::Debug for TrayHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrayHandle")
            .field("closed", &self.0.is_closed())
            .finish()
    }
}

/// The StatusNotifierItem implementation.
pub struct SnipunixTray {
    sender: std::sync::mpsc::Sender<TrayCommand>,
}

impl Tray for SnipunixTray {
    fn id(&self) -> String {
        "snipunix".to_string()
    }

    fn title(&self) -> String {
        "Snipunix".to_string()
    }

    fn icon_name(&self) -> String {
        "io.github.alokverma9.snipunix".to_string()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.sender.send(TrayCommand::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let item = |label: &str, command: TrayCommand| {
            let sender = self.sender.clone();
            StandardItem {
                label: label.to_string(),
                activate: Box::new(move |_| {
                    let _ = sender.send(command);
                }),
                ..Default::default()
            }
            .into()
        };
        vec![
            item(&tr!("Capture Region"), TrayCommand::CaptureRegion),
            item(&tr!("Capture Screen"), TrayCommand::CaptureScreen),
            MenuItem::Separator,
            item(&tr!("Show Snipunix"), TrayCommand::Show),
            item(&tr!("Settings…"), TrayCommand::Settings),
            MenuItem::Separator,
            item(&tr!("Quit"), TrayCommand::Quit),
        ]
    }
}

/// Starts the tray service.
///
/// The caller treats an error as "no tray on this desktop" and must not show a
/// modal error: capture, settings, and quit remain available elsewhere.
pub fn spawn(sender: std::sync::mpsc::Sender<TrayCommand>) -> Result<TrayHandle, String> {
    let tray = SnipunixTray { sender };
    tray.disable_dbus_name(ashpd::is_sandboxed())
        .assume_sni_available(true)
        .spawn()
        .map(TrayHandle)
        .map_err(|error| error.to_string())
}
