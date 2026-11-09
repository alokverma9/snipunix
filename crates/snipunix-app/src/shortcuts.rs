//! GlobalShortcuts portal prototype.
//!
//! The portal identifies callers by application ID. Sandboxed builds get an ID
//! from the sandbox. Host builds first try `org.freedesktop.host.portal.Registry`;
//! without an installed desktop entry the portal still rejects the caller, and
//! that refusal is reported verbatim instead of being hidden.

use std::str::FromStr;

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::{Error as AshpdError, PortalError, WindowIdentifier};
use futures_lite::StreamExt;

pub const REGION_SHORTCUT_ID: &str = "capture-region";
pub const SCREEN_SHORTCUT_ID: &str = "capture-screen";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundShortcut {
    pub id: String,
    pub description: String,
    pub trigger: String,
}

#[derive(Debug, Clone)]
pub enum ShortcutEvent {
    Registered(Vec<BoundShortcut>),
    Failed(String),
    Activated(String),
    Deactivated(String),
}

/// Creates a session, binds the two M0 shortcuts and forwards portal signals.
///
/// This future is intended to run on the GLib main context and never completes
/// while the session is alive.
pub async fn run(
    sender: async_channel::Sender<ShortcutEvent>,
    parent: Option<WindowIdentifier>,
    region_trigger: &str,
    screen_trigger: &str,
    stop: async_channel::Receiver<()>,
) -> Result<(), AshpdError> {
    register_host_identity().await;
    let proxy = GlobalShortcuts::new().await?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await?;
    let mut activated = proxy.receive_activated().await?;
    let mut deactivated = proxy.receive_deactivated().await?;

    let shortcuts = [
        NewShortcut::new(REGION_SHORTCUT_ID, "Capture a screen region")
            .preferred_trigger(region_trigger),
        NewShortcut::new(SCREEN_SHORTCUT_ID, "Capture the full screen")
            .preferred_trigger(screen_trigger),
    ];
    let request = proxy
        .bind_shortcuts(
            &session,
            &shortcuts,
            parent.as_ref(),
            BindShortcutsOptions::default(),
        )
        .await?;
    let bound = request.response()?;
    let descriptions = bound
        .shortcuts()
        .iter()
        .map(|shortcut| BoundShortcut {
            id: shortcut.id().to_string(),
            description: shortcut.description().to_string(),
            trigger: shortcut.trigger_description().to_string(),
        })
        .collect();
    let _ = sender.send(ShortcutEvent::Registered(descriptions)).await;

    enum Signal {
        Activated(String),
        Deactivated(String),
        Stop,
    }

    loop {
        let event = futures_lite::future::or(
            futures_lite::future::or(
                async {
                    activated
                        .next()
                        .await
                        .map(|signal| Signal::Activated(signal.shortcut_id().to_string()))
                },
                async {
                    deactivated
                        .next()
                        .await
                        .map(|signal| Signal::Deactivated(signal.shortcut_id().to_string()))
                },
            ),
            async { stop.recv().await.ok().map(|()| Signal::Stop) },
        )
        .await;

        match event {
            Some(Signal::Activated(id)) => {
                if sender.send(ShortcutEvent::Activated(id)).await.is_err() {
                    break;
                }
            }
            Some(Signal::Deactivated(id)) => {
                if sender.send(ShortcutEvent::Deactivated(id)).await.is_err() {
                    break;
                }
            }
            Some(Signal::Stop) | None => break,
        }
    }
    Ok(())
}

/// Registers this host process with the portal so the caller has an app ID.
///
/// Flatpak and Snap builds already have an identity. Host builds need an
/// installed `io.github.alokverma9.snipunix.desktop` entry and the
/// `org.freedesktop.host.portal.Registry` interface (xdg-desktop-portal 1.18+).
/// Failure is non-fatal: the portal call below reports the actionable error.
async fn register_host_identity() {
    if ashpd::is_sandboxed() {
        return;
    }
    match ashpd::AppID::from_str(crate::app::APP_ID) {
        Ok(app_id) => match ashpd::register_host_app(app_id).await {
            Ok(()) => tracing::debug!("registered host application ID with the portal"),
            Err(error) => {
                tracing::debug!(%error, "host application ID registration failed; continuing");
            }
        },
        Err(error) => tracing::warn!(%error, "invalid application ID"),
    }
}

/// Human-readable explanation, including the exact portal refusal when present.
#[must_use]
pub fn describe_error(error: &AshpdError) -> String {
    match error {
        AshpdError::Portal(PortalError::NotAllowed(message)) => format!(
            "The GlobalShortcuts portal refused the request: {message}. \
             Global shortcuts require an installed application ID. Install the desktop \
             entry or run the Flatpak build, then try again."
        ),
        AshpdError::PortalNotFound(interface) => format!(
            "No GlobalShortcuts portal is available ({interface}). Use a desktop shortcut \
             that runs `snipunix --capture region` instead."
        ),
        AshpdError::Response(ashpd::desktop::ResponseError::Cancelled) => {
            "Global shortcut registration was cancelled.".to_string()
        }
        other => format!(
            "Global shortcut registration failed: {other}. Use a desktop \
                          shortcut that runs `snipunix --capture region` instead."
        ),
    }
}
