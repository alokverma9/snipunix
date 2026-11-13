//! freedesktop Screenshot portal backend.

use std::time::Duration;

use ashpd::desktop::screenshot::{Screenshot, ScreenshotProxy};
use ashpd::{Error as AshpdError, PortalError};
use futures_lite::future;
use thiserror::Error;
use tracing::{info, warn};

use crate::image::{ImageError, decode_image};
use crate::uri::{UriError, portal_uri_to_file};
use crate::{CaptureBackend, CaptureFrame, CaptureFuture, CaptureOptions};

/// The portal can hang when a backend is wedged. Match the value used by
/// existing screenshot tools (Flameshot) and fail loudly after this long.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Screenshot portal backend using `interactive=false`.
#[derive(Debug, Clone)]
pub struct PortalScreenshot {
    timeout: Duration,
}

impl Default for PortalScreenshot {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT)
    }
}

impl PortalScreenshot {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl CaptureBackend for PortalScreenshot {
    fn capture(&self, options: CaptureOptions) -> CaptureFuture<'_> {
        Box::pin(async move {
            if options.window.is_some() {
                return Err(CaptureError::Unsupported(
                    "window capture is only available through the X11 backend".to_string(),
                ));
            }
            let request = Screenshot::request()
                .interactive(options.interactive)
                .modal(false)
                .identifier(options.parent);

            let send = request.send();
            let result = future::or(async move { Some(send.await) }, async move {
                async_io::Timer::after(self.timeout).await;
                None
            })
            .await;

            let request = match result {
                Some(Ok(request)) => request,
                Some(Err(error)) => return Err(map_error(error)),
                None => return Err(CaptureError::Timeout(self.timeout)),
            };

            let screenshot = request.response().map_err(map_error)?;
            let uri = screenshot.uri().as_str().to_string();
            let file = portal_uri_to_file(&uri).map_err(CaptureError::Uri)?;
            info!(path = %file.path.display(), "portal returned screenshot file");

            let bytes = std::fs::read(&file.path).map_err(|source| CaptureError::Read {
                path: file.path.clone(),
                source,
            })?;

            if file.should_remove_after_read() {
                if let Err(error) = std::fs::remove_file(&file.path) {
                    warn!(
                        path = %file.path.display(),
                        %error,
                        "could not remove the portal-created screenshot file"
                    );
                }
            }

            decode_image(&bytes, Some(uri))
                .map(CaptureFrame::new)
                .map_err(CaptureError::Image)
        })
    }
}

pub(crate) fn map_error(error: AshpdError) -> CaptureError {
    match error {
        AshpdError::Response(ashpd::desktop::ResponseError::Cancelled) => CaptureError::Cancelled,
        AshpdError::Response(ashpd::desktop::ResponseError::Other) => {
            CaptureError::PermissionDenied
        }
        AshpdError::Portal(PortalError::NotAllowed(message)) => {
            CaptureError::PermissionDeniedWithDetail(message)
        }
        AshpdError::Portal(PortalError::Cancelled(message)) => {
            CaptureError::CancelledWithDetail(message)
        }
        AshpdError::Portal(error) => CaptureError::Portal(error.to_string()),
        AshpdError::PortalNotFound(interface) => {
            CaptureError::PortalUnavailable(interface.to_string())
        }
        other => CaptureError::Portal(other.to_string()),
    }
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("no screenshot portal is available: {0}")]
    PortalUnavailable(String),
    #[error("screenshot permission was denied or has not been granted")]
    PermissionDenied,
    #[error("screenshot permission was denied: {0}")]
    PermissionDeniedWithDetail(String),
    #[error("screenshot request was cancelled")]
    Cancelled,
    #[error("screenshot request was cancelled: {0}")]
    CancelledWithDetail(String),
    #[error("screenshot portal timed out after {0:?}")]
    Timeout(Duration),
    #[error("screenshot portal failed: {0}")]
    Portal(String),
    #[error("portal returned an unusable URI: {0}")]
    Uri(#[from] UriError),
    #[error("failed to read captured screenshot at {path}: {source}")]
    Read {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode captured screenshot: {0}")]
    Image(#[source] ImageError),
    #[error("X11 capture is unavailable: {0}")]
    X11Unavailable(String),
    #[error("X11 capture failed: {0}")]
    X11(String),
    #[error("capture is not supported by this backend: {0}")]
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalInfo {
    /// Version advertised by `org.freedesktop.portal.Screenshot`.
    pub version: u32,
    /// `AvailableTargets` bits, present only on v3+ backends.
    pub available_targets: Option<u32>,
}

/// Reads the portal version and, when supported, the available capture targets.
pub async fn probe_portal() -> Result<PortalInfo, CaptureError> {
    let proxy = ScreenshotProxy::new().await.map_err(map_error)?;
    let version = proxy.version();
    let available_targets = proxy.available_targets().await.ok().map(|bits| bits.bits());
    Ok(PortalInfo {
        version,
        available_targets,
    })
}
