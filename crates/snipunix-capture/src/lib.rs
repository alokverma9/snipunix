//! Screenshot capture for Snipunix.
//!
//! M0 implements one backend: the freedesktop Screenshot portal with
//! `interactive=false`. The trait exists so the X11 and ScreenCast backends can
//! be added without changing the overlay or application.

#![forbid(unsafe_code)]

pub mod image;
pub mod portal;
pub mod uri;
pub mod x11;
pub mod x11_hotkey;

pub use image::{CapturedImage, ImageError, decode_image};
pub use portal::{CaptureError, PortalInfo, PortalScreenshot, probe_portal};
pub use uri::{PortalFile, portal_uri_to_file};
pub use x11::{PixelMonitor, X11Capture, X11Window};

use std::future::Future;
use std::pin::Pin;

use ashpd::WindowIdentifier;
use snipunix_core::ImageSize;

/// Options for one capture request.
#[derive(Debug, Default)]
pub struct CaptureOptions {
    /// `true` asks the compositor to show its own picker. Snipunix uses
    /// `false` and draws its own frozen-frame overlay.
    pub interactive: bool,
    /// Optional parent window handle for portal dialogs.
    pub parent: Option<WindowIdentifier>,
    /// Capture one specific window (X11 only). `None` captures the desktop.
    pub window: Option<u32>,
    /// Draw the pointer into the captured image (X11 only).
    pub include_cursor: bool,
}

/// One decoded capture plus the exact monitor pixel rectangles, when the
/// backend knows them.
#[derive(Debug, Clone)]
pub struct CaptureFrame {
    pub image: CapturedImage,
    /// Exact monitor rectangles in image pixels. Empty for composited portal
    /// captures, which fall back to a uniform scale.
    pub monitors: Vec<PixelMonitor>,
}

impl CaptureFrame {
    #[must_use]
    pub fn new(image: CapturedImage) -> Self {
        Self {
            image,
            monitors: Vec::new(),
        }
    }

    #[must_use]
    pub fn size(&self) -> ImageSize {
        self.image.size()
    }

    #[must_use]
    pub fn monitor_images(&self) -> Vec<snipunix_core::MonitorImage> {
        self.monitors
            .iter()
            .enumerate()
            .map(|(id, monitor)| monitor.image_rect_with_id(id as u32))
            .collect()
    }
}

/// Result of one capture request.
pub type CaptureFuture<'a> = Pin<Box<dyn Future<Output = Result<CaptureFrame, CaptureError>> + 'a>>;

/// A source of desktop and window screenshots.
pub trait CaptureBackend {
    fn capture(&self, options: CaptureOptions) -> CaptureFuture<'_>;
}

/// Which capture implementation to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendKind {
    #[default]
    Auto,
    Portal,
    X11,
}

impl BackendKind {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "portal" | "wayland" => Some(Self::Portal),
            "x11" => Some(Self::X11),
            _ => None,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Portal => "portal",
            Self::X11 => "x11",
        }
    }
}

/// `true` when the process is running on an X11 session with a usable display.
#[must_use]
pub fn is_x11_session() -> bool {
    std::env::var_os("DISPLAY").is_some() && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

/// The backend auto-selection would use on this session.
#[must_use]
pub fn detected_backend() -> BackendKind {
    if is_x11_session() {
        BackendKind::X11
    } else {
        BackendKind::Portal
    }
}

/// A backend that picks X11 on X11 sessions and the portal otherwise, falling
/// back to the other one when the first is unavailable.
pub struct AutoCapture {
    preference: BackendKind,
}

impl Default for AutoCapture {
    fn default() -> Self {
        Self::new(BackendKind::Auto)
    }
}

impl AutoCapture {
    #[must_use]
    pub fn new(preference: BackendKind) -> Self {
        Self { preference }
    }

    #[must_use]
    pub fn preference(&self) -> BackendKind {
        self.preference
    }
}

impl CaptureBackend for AutoCapture {
    fn capture(&self, options: CaptureOptions) -> CaptureFuture<'_> {
        Box::pin(async move {
            let first = match self.preference {
                // Window IDs only exist in the X11 backend, so auto selection
                // must not waste a portal request that is guaranteed to fail.
                BackendKind::Auto if options.window.is_some() => BackendKind::X11,
                BackendKind::Auto => detected_backend(),
                other => other,
            };
            let second = match first {
                BackendKind::X11 => BackendKind::Portal,
                _ => BackendKind::X11,
            };

            let fallback_options = CaptureOptions {
                interactive: options.interactive,
                // A portal parent handle belongs to the portal; an X11
                // fallback has no use for it.
                parent: None,
                window: options.window,
                include_cursor: options.include_cursor,
            };
            let primary = match first {
                BackendKind::X11 => X11Capture::new().capture(options).await,
                _ => PortalScreenshot::default().capture(options).await,
            };
            match primary {
                Ok(frame) => Ok(frame),
                Err(error) if should_fall_back(&error) => {
                    tracing::warn!(%error, fallback = second.label(), "capture backend failed; trying fallback");
                    match second {
                        BackendKind::X11 => X11Capture::new().capture(fallback_options).await,
                        _ => PortalScreenshot::default().capture(fallback_options).await,
                    }
                }
                Err(error) => Err(error),
            }
        })
    }
}

fn should_fall_back(error: &CaptureError) -> bool {
    matches!(
        error,
        CaptureError::PortalUnavailable(_)
            | CaptureError::X11Unavailable(_)
            | CaptureError::X11(_)
            | CaptureError::Timeout(_)
            | CaptureError::Unsupported(_)
    )
}
