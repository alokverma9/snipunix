//! Minimal command-line parser.
//!
//! M0 deliberately avoids a parsing crate: the surface is small and keeping the
//! dependency list short matters more than convenience here.

use std::path::PathBuf;

use snipunix_capture::BackendKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    Screen,
    Region,
    Window,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    pub target: Option<CaptureTarget>,
    pub output: Option<PathBuf>,
    pub exit_after: bool,
    pub register_shortcuts: bool,
    pub portal_info: bool,
    /// Countdown in seconds (0–10) before the screen is frozen.
    pub delay: u32,
    /// Recognise text in an existing image and print it.
    pub ocr: Option<PathBuf>,
    /// Explicit OCR model directory.
    pub ocr_models: Option<PathBuf>,
    /// Join recognised lines with spaces instead of newlines.
    pub remove_line_breaks: bool,
    /// Explicitly download the pinned OCR models and exit.
    pub download_ocr_models: bool,
    /// Capture backend preference.
    pub backend: BackendKind,
    /// X11 window ID for `--capture window`.
    pub window: Option<u32>,
    /// Draw the pointer into the capture (X11).
    pub include_cursor: bool,
    /// List capturable X11 windows and exit.
    pub list_windows: bool,
    /// Print a telemetry-free diagnostics report and exit.
    pub diagnostics: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseResult {
    Run(Cli),
    Help,
    Version,
}

fn parse_window_id(value: &str) -> Result<u32, String> {
    let trimmed = value.trim();
    let parsed = if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16)
    } else {
        trimmed.parse::<u32>()
    };
    parsed.map_err(|_| format!("invalid window ID '{value}'; expected decimal or 0x-prefixed hex"))
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<ParseResult, String> {
    let mut cli = Cli::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(ParseResult::Help),
            "--version" | "-V" => return Ok(ParseResult::Version),
            "--exit" => cli.exit_after = true,
            "--register-shortcuts" => cli.register_shortcuts = true,
            "--portal-info" => cli.portal_info = true,
            "--remove-line-breaks" => cli.remove_line_breaks = true,
            "--download-ocr-models" => cli.download_ocr_models = true,
            "--include-cursor" => cli.include_cursor = true,
            "--list-windows" => cli.list_windows = true,
            "--diagnostics" => cli.diagnostics = true,
            "--backend" => {
                let value = args.next().ok_or_else(|| {
                    "--backend requires a value: auto, portal, or x11".to_string()
                })?;
                cli.backend = BackendKind::parse(&value).ok_or_else(|| {
                    format!("unknown backend '{value}'; expected auto, portal, or x11")
                })?;
            }
            "--window" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--window requires an X11 window ID".to_string())?;
                cli.window = Some(parse_window_id(&value)?);
            }
            "--ocr" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--ocr requires an image path".to_string())?;
                cli.ocr = Some(PathBuf::from(value));
            }
            "--ocr-models" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--ocr-models requires a directory".to_string())?;
                cli.ocr_models = Some(PathBuf::from(value));
            }
            "--delay" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--delay requires a number of seconds (0–10)".to_string())?;
                let seconds: u32 = value.parse().map_err(|_| {
                    format!("invalid delay '{value}'; expected a whole number from 0 to 10")
                })?;
                if seconds > 10 {
                    return Err(format!(
                        "delay {seconds} is out of range; expected 0 to 10 seconds"
                    ));
                }
                cli.delay = seconds;
            }
            "--capture" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--capture requires a value: screen or region".to_string())?;
                cli.target = Some(match value.as_str() {
                    "screen" | "full" | "monitor" => CaptureTarget::Screen,
                    "region" | "area" => CaptureTarget::Region,
                    "window" => CaptureTarget::Window,
                    other => {
                        return Err(format!(
                            "unknown capture target '{other}'; expected screen, region, or window"
                        ));
                    }
                });
            }
            "--output" | "-o" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--output requires a file path".to_string())?;
                cli.output = Some(PathBuf::from(value));
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }

    Ok(ParseResult::Run(cli))
}

pub const HELP: &str = "\
Snipunix — Screenshot. Annotate. Share.

USAGE:
    snipunix [OPTIONS]

OPTIONS:
    --capture <screen|region|window>  Capture immediately after startup.
    -o, --output <PATH>         Write the resulting PNG to PATH.
    --delay <SECONDS>           Wait 0–10 seconds before capturing (default 0).
    --backend <auto|portal|x11> Capture backend preference (default auto).
    --window <ID>               X11 window ID for `--capture window`.
    --list-windows              List capturable X11 windows and exit.
    --diagnostics               Print a telemetry-free diagnostics report.
    --include-cursor            Draw the pointer into the capture (X11).
    --exit                      Quit after the capture completes.
    --register-shortcuts        Register global shortcuts through the portal.
    --portal-info               Print Screenshot portal capabilities and exit.
    --ocr <PATH>                Recognise text in an image, print it, and exit.
    --ocr-models <DIR>          Use OCR models from DIR.
    --remove-line-breaks        Join OCR lines with spaces instead of newlines.
    --download-ocr-models       Download and verify the pinned OCR models.
    -h, --help                  Show this help.
    -V, --version               Show the version.

EXAMPLES:
    snipunix
    snipunix --capture screen --output /tmp/snipunix-screen.png
    snipunix --capture region --output /tmp/snipunix-region.png
    snipunix --capture region --delay 5
    snipunix --list-windows
    snipunix --capture window --window 0x400001 --output /tmp/window.png
";

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<ParseResult, String> {
        parse(args.iter().map(ToString::to_string))
    }

    #[test]
    fn parses_capture_screen_with_output() {
        let result = parse_args(&["--capture", "screen", "-o", "/tmp/a.png"]).unwrap();
        let ParseResult::Run(cli) = result else {
            panic!("expected run");
        };
        assert_eq!(cli.target, Some(CaptureTarget::Screen));
        assert_eq!(cli.output, Some(PathBuf::from("/tmp/a.png")));
    }

    #[test]
    fn parses_region_aliases() {
        for alias in ["region", "area"] {
            let result = parse_args(&["--capture", alias]).unwrap();
            let ParseResult::Run(cli) = result else {
                panic!("expected run");
            };
            assert_eq!(cli.target, Some(CaptureTarget::Region));
        }
    }

    #[test]
    fn rejects_missing_values_and_unknown_flags() {
        assert!(parse_args(&["--capture"]).is_err());
        assert!(parse_args(&["--output"]).is_err());
        assert!(parse_args(&["--nope"]).is_err());
        assert!(parse_args(&["--capture", "desktop"]).is_err());
    }

    #[test]
    fn parses_window_and_backend_options() {
        let ParseResult::Run(cli) = parse_args(&[
            "--capture",
            "window",
            "--window",
            "0x4a00007",
            "--backend",
            "x11",
            "--include-cursor",
        ])
        .unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.target, Some(CaptureTarget::Window));
        assert_eq!(cli.window, Some(0x4a0_0007));
        assert_eq!(cli.backend, BackendKind::X11);
        assert!(cli.include_cursor);

        let ParseResult::Run(cli) = parse_args(&["--window", "1234"]).unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.window, Some(1234));
        assert!(parse_args(&["--backend", "mir"]).is_err());
        assert!(parse_args(&["--window"]).is_err());
        assert!(parse_args(&["--window", "nope"]).is_err());
    }

    #[test]
    fn parses_delay_and_rejects_out_of_range_values() {
        let ParseResult::Run(cli) = parse_args(&["--capture", "region", "--delay", "5"]).unwrap()
        else {
            panic!("expected run");
        };
        assert_eq!(cli.delay, 5);
        assert!(parse_args(&["--delay", "11"]).is_err());
        assert!(parse_args(&["--delay", "-1"]).is_err());
        assert!(parse_args(&["--delay", "abc"]).is_err());
    }

    #[test]
    fn parses_ocr_options() {
        let ParseResult::Run(cli) = parse_args(&[
            "--ocr",
            "/tmp/shot.png",
            "--ocr-models",
            "/opt/models",
            "--remove-line-breaks",
        ])
        .unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.ocr, Some(PathBuf::from("/tmp/shot.png")));
        assert_eq!(cli.ocr_models, Some(PathBuf::from("/opt/models")));
        assert!(cli.remove_line_breaks);
        assert!(!cli.download_ocr_models);
        assert!(parse_args(&["--download-ocr-models"]).is_ok());
        assert!(parse_args(&["--ocr"]).is_err());
        assert!(parse_args(&["--ocr-models"]).is_err());
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parse_args(&["--help"]).unwrap(), ParseResult::Help);
        assert_eq!(parse_args(&["-V"]).unwrap(), ParseResult::Version);
    }
}
