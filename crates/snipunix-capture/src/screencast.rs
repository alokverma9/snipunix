//! freedesktop ScreenCast portal session for video recording.
//!
//! The portal returns a restricted PipeWire file descriptor and one or more
//! stream node IDs. The descriptor is only usable by this process, so the
//! recording engine consumes it directly instead of delegating to an external
//! recorder.

use std::os::fd::OwnedFd;
use std::str::FromStr;
use std::time::Duration;

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions,
};
use ashpd::desktop::{PersistMode, Session};
use futures_lite::future;
use tracing::info;

use crate::portal::{CaptureError, map_error};

/// A portal dialog can stay open while the user chooses a monitor or window.
/// Fail instead of hanging forever when the desktop is wedged.
pub const DEFAULT_SCREENCAST_TIMEOUT: Duration = Duration::from_secs(60);

/// Options for one ScreenCast session.
#[derive(Debug, Clone, Default)]
pub struct ScreencastOptions {
    /// Ask the compositor to embed the pointer in the stream.
    pub include_cursor: bool,
    /// Ask the portal to return a restore token for later sessions.
    pub persist: bool,
    /// Token from a previous session. A valid token skips source selection.
    pub restore_token: Option<String>,
    /// Which source kinds the user may pick.
    pub sources: SourceType,
    /// Allow selecting more than one source.
    pub multiple: bool,
}

impl ScreencastOptions {
    /// Monitor-or-window selection with an embedded cursor and persistence.
    #[must_use]
    pub fn recording(include_cursor: bool, restore_token: Option<String>) -> Self {
        Self {
            include_cursor,
            persist: true,
            restore_token,
            sources: SourceType::MONITOR | SourceType::WINDOW,
            multiple: false,
        }
    }
}

/// One negotiated PipeWire stream.
#[derive(Debug, Clone)]
pub struct CastStream {
    /// PipeWire node ID exposed by the portal.
    pub node_id: u32,
    /// Monotonic serial, available on portal v6+ and preferred over the node ID.
    pub serial: Option<u64>,
    /// Size in the compositor coordinate space (logical pixels).
    pub logical_size: Option<(i32, i32)>,
    /// Position in the compositor coordinate space.
    pub position: Option<(i32, i32)>,
    /// Monitor, window, or virtual source.
    pub kind: Option<SourceType>,
}

/// A live ScreenCast session.
///
/// Dropping the value closes the D-Bus session; the PipeWire fd stays open
/// until the recording pipeline has been torn down.
pub struct ScreencastSession {
    streams: Vec<CastStream>,
    restore_token: Option<String>,
    pipewire_fd: OwnedFd,
    session: Session<Screencast>,
    // Keep the proxy alive: it owns the D-Bus connection used by the session.
    _proxy: Screencast,
}

impl std::fmt::Debug for ScreencastSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreencastSession")
            .field("streams", &self.streams)
            .field("restore_token", &self.restore_token.is_some())
            .finish()
    }
}

impl ScreencastSession {
    #[must_use]
    pub fn streams(&self) -> &[CastStream] {
        &self.streams
    }

    /// The first stream, which is what single-source recording uses.
    #[must_use]
    pub fn primary_stream(&self) -> Option<&CastStream> {
        self.streams.first()
    }

    #[must_use]
    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    /// The restricted PipeWire remote descriptor.
    #[must_use]
    pub fn pipewire_fd(&self) -> &OwnedFd {
        &self.pipewire_fd
    }

    /// Closes the portal session and ends the compositor's capture stream.
    pub async fn close(&self) {
        if let Err(error) = self.session.close().await {
            tracing::warn!(%error, "could not close the ScreenCast session");
        }
    }
}

/// Registers this process as the installed host application.
///
/// The ScreenCast portal scopes persisted restore tokens to the caller's
/// application ID. Registering before opening a session lets the GUI and the
/// headless CLI share the same token instead of each showing a picker.
pub async fn register_host_identity(app_id: &str) {
    if ashpd::is_sandboxed() {
        return;
    }
    let Ok(app_id) = ashpd::AppID::from_str(app_id) else {
        tracing::warn!(app_id, "invalid application ID");
        return;
    };
    match ashpd::register_host_app(app_id).await {
        Ok(()) => tracing::debug!("registered host application ID with the portal"),
        Err(error) => {
            tracing::debug!(%error, "host application ID registration failed; continuing");
        }
    }
}

/// Opens a ScreenCast session and waits for the user's source selection.
pub async fn open(
    options: ScreencastOptions,
    timeout: Duration,
) -> Result<ScreencastSession, CaptureError> {
    open_with_timeout(options, timeout).await
}

async fn open_with_timeout(
    options: ScreencastOptions,
    timeout: Duration,
) -> Result<ScreencastSession, CaptureError> {
    let proxy = Screencast::new().await.map_err(map_error)?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(map_error)?;

    let cursor_mode = if options.include_cursor {
        CursorMode::EMBEDDED
    } else {
        CursorMode::HIDDEN
    };
    let persist_mode = if options.persist {
        PersistMode::Application
    } else {
        PersistMode::DoNot
    };
    let select = SelectSourcesOptions::default()
        .set_cursor_mode(cursor_mode)
        .set_sources(options.sources)
        .set_multiple(options.multiple)
        .set_persist_mode(persist_mode)
        .set_restore_token(options.restore_token.as_deref());

    with_timeout(
        proxy.select_sources(&session, select),
        timeout,
        "selecting a ScreenCast source",
    )
    .await?;

    let request = with_timeout(
        proxy.start(&session, None, StartCastOptions::default()),
        timeout,
        "waiting for the ScreenCast source picker",
    )
    .await?;
    let response = request.response().map_err(map_error)?;

    let streams = response
        .streams()
        .iter()
        .map(|stream| CastStream {
            node_id: stream.pipe_wire_node_id(),
            serial: stream.pipewire_serial(),
            logical_size: stream.size(),
            position: stream.position(),
            kind: stream.source_type(),
        })
        .collect::<Vec<_>>();

    if streams.is_empty() {
        return Err(CaptureError::Unsupported(
            "the ScreenCast portal returned no streams".to_string(),
        ));
    }

    let restore_token = response.restore_token().map(ToString::to_string);
    let pipewire_fd = with_timeout(
        proxy.open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions {}),
        timeout,
        "opening the PipeWire remote",
    )
    .await?;

    info!(
        streams = streams.len(),
        node = streams[0].node_id,
        size = ?streams[0].logical_size,
        restored = options.restore_token.is_some(),
        "ScreenCast session started"
    );

    Ok(ScreencastSession {
        streams,
        restore_token,
        pipewire_fd,
        session,
        _proxy: proxy,
    })
}

async fn with_timeout<T, F>(
    request: F,
    timeout: Duration,
    description: &'static str,
) -> Result<T, CaptureError>
where
    F: std::future::Future<Output = Result<T, ashpd::Error>>,
{
    match future::or(async { Some(request.await) }, async {
        async_io::Timer::after(timeout).await;
        None
    })
    .await
    {
        Some(result) => result.map_err(map_error),
        None => {
            tracing::warn!(?timeout, description, "ScreenCast portal timed out");
            Err(CaptureError::Timeout(timeout))
        }
    }
}

/// Capabilities advertised by the desktop's ScreenCast portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreencastInfo {
    pub version: u32,
    pub source_types: SourceType,
    pub cursor_modes: CursorMode,
}

/// Reads the portal version and supported sources without opening a session.
pub async fn probe_screencast() -> Result<ScreencastInfo, CaptureError> {
    let proxy = Screencast::new().await.map_err(map_error)?;
    let version = proxy.version();
    let source_types = proxy.available_source_types().await.map_err(map_error)?;
    let cursor_modes = proxy.available_cursor_modes().await.map_err(map_error)?;
    Ok(ScreencastInfo {
        version,
        source_types,
        cursor_modes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_defaults_ask_for_monitor_or_window_and_persist() {
        let options = ScreencastOptions::recording(true, Some("token".to_string()));
        assert!(options.include_cursor);
        assert!(options.persist);
        assert_eq!(options.restore_token.as_deref(), Some("token"));
        assert!(options.sources.contains(SourceType::MONITOR));
        assert!(options.sources.contains(SourceType::WINDOW));
        assert!(!options.multiple);
    }

    #[test]
    fn cursor_mode_follows_the_cursor_option() {
        let with = ScreencastOptions::recording(true, None);
        let without = ScreencastOptions::recording(false, None);
        assert!(with.include_cursor);
        assert!(!without.include_cursor);
    }

    #[test]
    fn stream_metadata_survives_conversion() {
        let stream = CastStream {
            node_id: 42,
            serial: Some(7),
            logical_size: Some((1920, 1080)),
            position: Some((0, 0)),
            kind: Some(SourceType::MONITOR),
        };
        assert_eq!(stream.node_id, 42);
        assert_eq!(stream.serial, Some(7));
        assert_eq!(stream.kind, Some(SourceType::MONITOR));
    }
}
