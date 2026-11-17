//! Local diagnostics reporting for Snipunix.
//!
//! # What a report contains
//!
//! A report describes the *environment* the application runs in: the desktop
//! session, the portal versions, the available encoders, the configured
//! settings, and the crash reports collected so far. It never contains
//! screenshot pixels, recognised text, clipboard content, file names from the
//! user's directories, or anything else the user produced.
//!
//! # What a report does not do
//!
//! Nothing in this crate opens a network connection. Reports are written to
//! a file the user chooses and are then attached to an issue by hand. This is
//! deliberate: the privacy statement promises that Snipunix does not talk to
//! the network, and the release gate verifies that promise with a packet
//! capture.

use std::path::{Path, PathBuf};

use snipunix_capture::BackendKind;

pub mod crash;

mod report;

pub use report::{DiagnosticsReport, ReportOptions, Section};

/// Builds a report for the current environment.
///
/// `options.backend` is the backend the user asked for, which may differ
/// from the backend the application would pick automatically.
#[must_use]
pub fn collect(options: &ReportOptions) -> DiagnosticsReport {
    report::collect(options)
}

/// Writes `report` to `path`, creating the parent directory when needed.
///
/// # Errors
///
/// Returns an error when the directory or the file cannot be written.
pub fn write_report(report: &DiagnosticsReport, path: &Path) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, report.to_text())
}

/// Default directory for exported reports.
///
/// `$XDG_STATE_HOME/snipunix/diagnostics` keeps the files with the rest of
/// the application's state rather than among the user's documents.
#[must_use]
pub fn default_export_directory() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/state")
        });
    base.join("snipunix").join("diagnostics")
}

/// The backend the user asked for, or the one that would be used.
#[must_use]
pub fn effective_backend(preference: BackendKind) -> BackendKind {
    match preference {
        BackendKind::Auto => snipunix_capture::detected_backend(),
        other => other,
    }
}
