//! Opt-in local crash capture.
//!
//! # Off by default
//!
//! Crash reports are **disabled** until the user turns them on in Settings.
//! While they are off, a panic is logged exactly as before and nothing is
//! written to disk. This is the default state, and the release gate checks
//! that a fresh installation leaves the crash directory empty.
//!
//! # What a crash report contains
//!
//! The timestamp, the application version, the panic message, the source
//! location, the thread name, and a backtrace. It never contains
//! screenshot pixels, recognised text, clipboard content, or file names.
//!
//! # Where the reports go
//!
//! `$XDG_STATE_HOME/snipunix/crashes`, one plain-text file per crash. The
//! directory is pruned to the newest [`DEFAULT_KEEP`] files so it cannot grow
//! without bound.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// How many crash reports are kept by default.
pub const DEFAULT_KEEP: usize = 10;

/// Whether crash reports are currently written to disk.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// How many reports to keep.
static KEEP: AtomicUsize = AtomicUsize::new(DEFAULT_KEEP);

/// Whether the panic hook has been installed.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// One recorded crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReport {
    /// Path of the report file.
    pub path: PathBuf,
    /// When the crash happened, in ISO 8601.
    pub timestamp: String,
    /// The panic message.
    pub message: String,
    /// The source location of the panic.
    pub location: String,
}

/// The directory crash reports are written to.
#[must_use]
pub fn directory() -> PathBuf {
    state_directory().join("crashes")
}

/// The state directory of the application.
#[must_use]
pub fn state_directory() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/state")
        });
    base.join("snipunix")
}

/// Turns crash reporting on or off.
///
/// Turning it on takes effect immediately; the panic hook is installed on the
/// first call so that a crash before the settings were read is still captured.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
    if enabled {
        install();
    }
}

/// Whether crash reporting is currently on.
#[must_use]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Sets how many reports are kept.
pub fn set_keep(count: usize) {
    KEEP.store(count.max(1), Ordering::SeqCst);
}

/// How many reports are kept.
#[must_use]
pub fn keep() -> usize {
    KEEP.load(Ordering::SeqCst)
}

/// Installs the panic hook.
///
/// The hook chains to the previous one, so the application's usual logging
/// still happens. It is idempotent.
pub fn install() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if ENABLED.load(Ordering::SeqCst) {
            // A failure inside the hook must not abort the process, so every
            // step is best-effort.
            let _ = write_report(info);
        }
        previous(info);
    }));
}

/// Writes the report for a panic. Returns the file that was written.
fn write_report(info: &std::panic::PanicHookInfo<'_>) -> std::io::Result<PathBuf> {
    let directory = directory();
    std::fs::create_dir_all(&directory)?;

    let message = payload_text(info);
    let location = info
        .location()
        .map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        })
        .unwrap_or_else(|| "unknown".to_string());
    let timestamp = timestamp();
    let name = format!("crash-{timestamp}-{}.txt", std::process::id());
    let path = directory.join(name);

    let mut contents = String::new();
    contents.push_str("snipunix crash report\n");
    contents.push_str(&format!("timestamp: {timestamp}\n"));
    contents.push_str(&format!("version: {}\n", env!("CARGO_PKG_VERSION")));
    contents.push_str(&format!(
        "platform: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    contents.push_str(&format!("thread: {}\n", thread_name()));
    contents.push_str(&format!("location: {location}\n"));
    contents.push_str(&format!("panic: {message}\n"));
    contents.push_str("\nbacktrace:\n");
    let backtrace = std::backtrace::Backtrace::force_capture();
    contents.push_str(&format!("{backtrace}\n"));

    std::fs::write(&path, contents)?;
    let _ = prune(&directory, keep());
    Ok(path)
}

/// Extracts the panic message from the payload.
fn payload_text(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
}

/// The name of the thread that crashed.
fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string()
}

/// The current time in ISO 8601, without pulling in a date-time crate.
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_unix(seconds)
}

/// Formats a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
fn format_unix(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Converts a day count since the Unix epoch into a calendar date.
///
/// This is Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Lists the crash reports in `directory`, oldest first.
pub fn list_reports(directory: &Path) -> std::io::Result<Vec<CrashReport>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut reports = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("txt") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut timestamp = String::new();
        let mut message = String::new();
        let mut location = String::new();
        for line in contents.lines() {
            if let Some(value) = line.strip_prefix("timestamp: ") {
                timestamp = value.to_string();
            } else if let Some(value) = line.strip_prefix("panic: ") {
                message = value.to_string();
            } else if let Some(value) = line.strip_prefix("location: ") {
                location = value.to_string();
            }
        }
        reports.push(CrashReport {
            path,
            timestamp,
            message,
            location,
        });
    }
    reports.sort_by(|left, right| right.path.cmp(&left.path));
    reports.reverse();
    Ok(reports)
}

/// Removes the oldest reports so that at most `keep` remain.
///
/// Returns the number of files that were removed.
pub fn prune(directory: &Path, keep: usize) -> std::io::Result<usize> {
    let mut reports = list_reports(directory)?;
    if reports.len() <= keep {
        return Ok(0);
    }
    let mut removed = 0;
    for report in reports.drain(..reports.len() - keep) {
        if std::fs::remove_file(&report.path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Deletes every crash report in `directory`.
///
/// Returns the number of files that were removed.
pub fn clear(directory: &Path) -> std::io::Result<usize> {
    prune(directory, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_timestamps() {
        assert_eq!(format_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(format_unix(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn lists_and_prunes_reports() {
        let directory = std::env::temp_dir().join(format!(
            "snipunix-crash-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        for index in 0..5 {
            std::fs::write(
                directory.join(format!("crash-{index}.txt")),
                format!(
                    "timestamp: 2026-01-0{}T00:00:00Z\npanic: boom {index}\n",
                    index + 1
                ),
            )
            .unwrap();
        }
        let reports = list_reports(&directory).unwrap();
        assert_eq!(reports.len(), 5);
        assert!(
            reports
                .iter()
                .all(|report| report.message.starts_with("boom"))
        );
        assert_eq!(reports[0].location, "");

        assert_eq!(prune(&directory, 2).unwrap(), 3);
        assert_eq!(list_reports(&directory).unwrap().len(), 2);

        assert_eq!(clear(&directory).unwrap(), 2);
        assert!(list_reports(&directory).unwrap().is_empty());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn listing_a_missing_directory_is_not_an_error() {
        let missing = std::env::temp_dir().join("snipunix-crash-missing-directory");
        assert!(list_reports(&missing).unwrap().is_empty());
    }

    #[test]
    fn crash_reporting_is_off_by_default() {
        // The global starts disabled; a test process that never enables it
        // must not write anything.
        assert!(!is_enabled());
    }
}
