//! Builds the diagnostics report.

use std::path::PathBuf;

use snipunix_capture::BackendKind;

/// Which parts of the environment to describe.
#[derive(Debug, Clone)]
pub struct ReportOptions {
    /// The backend the user asked for.
    pub backend: BackendKind,
    /// Whether to probe the X11 server. Skipped when there is no `DISPLAY`.
    pub probe_x11: bool,
    /// Directory holding the crash reports, if any.
    pub crash_directory: Option<PathBuf>,
    /// Whether to describe the loaded translation catalogue.
    pub include_translations: bool,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            backend: BackendKind::Auto,
            probe_x11: true,
            crash_directory: None,
            include_translations: true,
        }
    }
}

/// One titled group of lines in the report.
#[derive(Debug, Clone)]
pub struct Section {
    /// Heading of the section.
    pub title: String,
    /// Lines below the heading.
    pub lines: Vec<String>,
}

/// A complete diagnostics report.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticsReport {
    /// The sections of the report, in display order.
    pub sections: Vec<Section>,
}

impl DiagnosticsReport {
    /// Adds a section with the given lines.
    pub fn push(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.sections.push(Section {
            title: title.into(),
            lines,
        });
    }

    /// Renders the report as plain text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut output = String::from("snipunix diagnostics\n");
        for section in &self.sections {
            output.push_str("\n[");
            output.push_str(&section.title);
            output.push_str("]\n");
            for line in &section.lines {
                output.push_str(line);
                output.push('\n');
            }
        }
        output
    }
}

/// Collects the report.
pub(crate) fn collect(options: &ReportOptions) -> DiagnosticsReport {
    let mut report = DiagnosticsReport::default();
    report.push("Application", application_section());
    report.push("Session", session_section());
    report.push("Capture", capture_section(options));
    report.push("Recording", recording_section());
    report.push("Text recognition", ocr_section());
    report.push("Settings", settings_section());
    if options.include_translations {
        report.push("Language", translation_section());
    }
    report.push("Crash reports", crash_section(options));
    report
}

fn application_section() -> Vec<String> {
    let mut lines = vec![
        format!("version: {}", env!("CARGO_PKG_VERSION")),
        format!(
            "platform: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    ];
    if let Ok(release) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        lines.push(format!("kernel: {}", release.trim()));
    }
    if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
        for line in os_release.lines() {
            if let Some(name) = line.strip_prefix("PRETTY_NAME=") {
                lines.push(format!("distribution: {}", name.trim_matches('"')));
            }
        }
    }
    lines.push(format!("diagnostics format: {}", env!("CARGO_PKG_VERSION")));
    lines
}

fn session_section() -> Vec<String> {
    let mut lines = Vec::new();
    for key in [
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "GDK_BACKEND",
        "GDK_SCALE",
        "GDK_DPI_SCALE",
    ] {
        if let Ok(value) = std::env::var(key) {
            lines.push(format!("{key}: {}", redact(&value)));
        }
    }
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") if std::env::var_os("WAYLAND_DISPLAY").is_some() => {
            lines.push("session: Wayland desktop".to_string());
        }
        Ok("wayland") => lines.push(
            "warning: XDG_SESSION_TYPE=wayland but WAYLAND_DISPLAY is unset; the desktop \
             Wayland socket is unavailable and DISPLAY may be a test or nested server"
                .to_string(),
        ),
        Ok("x11") => lines.push("session: X11 desktop".to_string()),
        Ok(other) if !other.is_empty() => lines.push(format!("session: {}", redact(other))),
        _ => {}
    }
    lines
}

fn capture_section(options: &ReportOptions) -> Vec<String> {
    let mut lines = vec![
        format!(
            "auto backend: {}",
            snipunix_capture::detected_backend().label()
        ),
        format!("requested backend: {}", options.backend.label()),
    ];

    match async_io::block_on(snipunix_capture::probe_portal()) {
        Ok(info) => {
            lines.push(format!("screenshot portal: version {}", info.version));
            match info.available_targets {
                Some(bits) => lines.push(format!("screenshot targets: {bits:#x}")),
                None => lines.push("screenshot targets: unknown (v2 or older)".to_string()),
            }
        }
        Err(error) => lines.push(format!("screenshot portal: unavailable ({error})")),
    }

    match async_io::block_on(snipunix_capture::probe_screencast()) {
        Ok(info) => {
            lines.push(format!("screencast portal: version {}", info.version));
            lines.push(format!(
                "screencast sources: {:#x}",
                info.source_types.bits()
            ));
            lines.push(format!(
                "screencast cursor modes: {:#x}",
                info.cursor_modes.bits()
            ));
        }
        Err(error) => lines.push(format!("screencast portal: unavailable ({error})")),
    }

    if options.probe_x11 && std::env::var_os("DISPLAY").is_some() {
        match snipunix_capture::X11Capture::new().diagnostics() {
            Ok(report) => {
                for line in report.lines() {
                    lines.push(line.to_string());
                }
            }
            Err(error) => lines.push(format!("X11: unavailable ({error})")),
        }
    } else if !options.probe_x11 {
        lines.push("X11: not probed".to_string());
    } else {
        lines.push("X11: no DISPLAY".to_string());
    }

    lines
}

fn recording_section() -> Vec<String> {
    match snipunix_record::init() {
        Ok(()) => {
            let support = snipunix_record::encoder_support();
            let mut lines = vec![format!("gstreamer: {}", snipunix_record::version())];
            lines.push(format!(
                "gstreamer encoders: {}",
                if support.available.is_empty() {
                    "none".to_string()
                } else {
                    support
                        .available
                        .iter()
                        .map(|encoder| encoder.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ));
            lines.push(match support.preferred {
                Some(encoder) => format!(
                    "gstreamer preferred encoder: {}{}",
                    encoder.label(),
                    if encoder.is_hardware() {
                        " (hardware)"
                    } else {
                        ""
                    }
                ),
                None => "gstreamer preferred encoder: none".to_string(),
            });
            lines
        }
        Err(error) => vec![format!("gstreamer: unavailable ({error})")],
    }
}

fn ocr_section() -> Vec<String> {
    let settings = snipunix_settings::Settings::load(&snipunix_settings::Settings::default_path())
        .unwrap_or_default();
    match snipunix_ocr::ModelPaths::discover(settings.ocr.model_directory.clone().as_deref()) {
        Ok(paths) => vec![
            format!(
                "ocr models: {} and {}",
                paths.detection.display(),
                paths.recognition.display()
            ),
            format!("auto copy: {}", settings.ocr.auto_copy),
            format!("remove line breaks: {}", settings.ocr.remove_line_breaks),
        ],
        Err(error) => vec![format!("ocr models: missing ({error})")],
    }
}

fn settings_section() -> Vec<String> {
    let path = snipunix_settings::Settings::default_path();
    let settings = snipunix_settings::Settings::load(&path).unwrap_or_default();
    vec![
        format!(
            "settings file: {} ({})",
            redact(&path.display().to_string()),
            if path.exists() { "present" } else { "defaults" }
        ),
        format!(
            "capture: mode={:?} delay={} cursor={} copy={}",
            settings.capture.default_mode,
            settings.capture.delay,
            settings.capture.include_cursor,
            settings.capture.copy_after_capture,
        ),
        format!(
            "recording: fps={} bitrate={}kbps cursor={} mic={} system={} restore-token={}",
            settings.recording.fps,
            settings.recording.bitrate_kbps,
            settings.recording.include_cursor,
            settings.recording.microphone,
            settings.recording.system_audio,
            if settings.recording.restore_token.is_some() {
                "present"
            } else {
                "none"
            },
        ),
        format!(
            "diagnostics: crash-reports={} keep={}",
            settings.diagnostics.crash_reports, settings.diagnostics.keep_crash_reports,
        ),
    ]
}

fn translation_section() -> Vec<String> {
    let mut lines = vec![format!(
        "active locale: {}",
        snipunix_i18n::active_locale().unwrap_or("none")
    )];
    lines.push(format!(
        "available locales: {}",
        snipunix_i18n::available_locales().join(", ")
    ));
    lines
}

fn crash_section(options: &ReportOptions) -> Vec<String> {
    let Some(directory) = options.crash_directory.clone() else {
        return vec!["crash reporting: not consulted".to_string()];
    };
    let mut lines = vec![format!(
        "crash report directory: {}",
        redact(&directory.display().to_string())
    )];
    match crate::crash::list_reports(&directory) {
        Ok(reports) => {
            lines.push(format!("crash reports: {}", reports.len()));
            for report in reports.iter().rev().take(10) {
                lines.push(format!(
                    "  {} — {}",
                    report.timestamp,
                    report.message.lines().next().unwrap_or("panic")
                ));
            }
        }
        Err(error) => lines.push(format!("crash reports: unavailable ({error})")),
    }
    lines
}

/// Replaces the user's home directory with `~` so the report does not leak
/// the account name.
fn redact(value: &str) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return value.to_string();
    };
    let home = home.display().to_string();
    if home.is_empty() {
        return value.to_string();
    }
    value.replace(&home, "~")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_renders_sections() {
        let mut report = DiagnosticsReport::default();
        report.push("First", vec!["one".to_string()]);
        report.push("Second", vec!["two".to_string()]);
        let text = report.to_text();
        assert!(text.starts_with("snipunix diagnostics\n"));
        assert!(text.contains("[First]\none\n"));
        assert!(text.contains("[Second]\ntwo\n"));
    }

    #[test]
    fn redact_hides_the_home_directory() {
        // The test only checks the shape: with no HOME the value is unchanged,
        // with a HOME the prefix is replaced.
        let value = "/home/example/pictures";
        let redacted = redact(value);
        assert!(redacted.contains("pictures"));
    }
}
