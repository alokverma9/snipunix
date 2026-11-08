//! Screenshot capture for Snipunix.
//!
//! M0 implements one backend: the freedesktop Screenshot portal with
//! `interactive=false`. The trait exists so the X11 and ScreenCast backends can
//! be added without changing the overlay or application.

#![forbid(unsafe_code)]

pub mod image;
pub mod portal;
pub mod uri;

pub use image::{CapturedImage, ImageError, decode_image};
pub use portal::{CaptureError, PortalInfo, PortalScreenshot, probe_portal};
pub use uri::{PortalFile, portal_uri_to_file};

use std::future::Future;
use std::pin::Pin;

use ashpd::WindowIdentifier;

/// Options for one capture request.
#[derive(Default)]
pub struct CaptureOptions {
    /// `true` asks the compositor to show its own picker. Snipunix uses
    /// `false` and draws its own frozen-frame overlay.
    pub interactive: bool,
    /// Optional parent window handle for portal dialogs.
    pub parent: Option<WindowIdentifier>,
}

pub type CaptureFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CapturedImage, CaptureError>> + 'a>>;

/// A source of full-desktop screenshots.
pub trait CaptureBackend {
    fn capture(&self, options: CaptureOptions) -> CaptureFuture<'_>;
}
