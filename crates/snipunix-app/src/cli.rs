//! Minimal command-line parser.
//!
//! M0 deliberately avoids a parsing crate: the surface is small and keeping the
//! dependency list short matters more than convenience here.

use std::path::PathBuf;

use snipunix_capture::BackendKind;
use snipunix_record::LogicalCrop;

/// Audio sources for a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordAudio {
    #[default]
    None,
    Microphone,
    System,
    Both,
}

impl RecordAudio {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" | "off" => Some(Self::None),
            "mic" | "microphone" => Some(Self::Microphone),
            "system" | "monitor" => Some(Self::System),
            "both" | "mic+system" => Some(Self::Both),
            _ => None,
        }
    }

    #[must_use]
    pub fn options(self) -> snipunix_record::AudioOptions {
        match self {
            Self::None => snipunix_record::AudioOptions::default(),
            Self::Microphone => snipunix_record::AudioOptions {
                microphone: true,
                system: false,
            },
            Self::System => snipunix_record::AudioOptions {
                microphone: false,
                system: true,
            },
            Self::Both => snipunix_record::AudioOptions {
                microphone: true,
                system: true,
            },
        }
    }
}

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
    /// Write a diagnostics report to a file and exit.
    pub diagnostics_output: Option<PathBuf>,
    /// Record a screen, region, or window instead of taking a screenshot.
    pub record: Option<CaptureTarget>,
    /// Stop recording after this many seconds. `0` records until Ctrl+C.
    pub record_duration: u32,
    /// Recording frame rate override.
    pub record_fps: Option<u32>,
    /// Recording output path. Defaults to the recording directory.
    pub record_output: Option<PathBuf>,
    /// Region inside the selected monitor, in logical pixels.
    pub record_region: Option<LogicalCrop>,
    /// Audio sources to record.
    pub record_audio: Option<RecordAudio>,
    /// Leave the pointer out of the recording.
    pub record_no_cursor: bool,
    /// Encode a synthetic pattern to verify the GStreamer setup.
    pub record_selftest: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseResult {
    Run(Cli),
    Help,
    Version,
}

fn parse_region(value: &str) -> Result<LogicalCrop, String> {
    let parts = value
        .split(',')
        .map(str::trim)
        .map(|part| {
            part.parse::<i32>()
                .map_err(|_| format!("invalid recording region '{value}'; expected X,Y,W,H"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let [x, y, width, height] = parts.as_slice() else {
        return Err(format!(
            "invalid recording region '{value}'; expected exactly four values X,Y,W,H"
        ));
    };
    LogicalCrop::new(*x, *y, *width, *height).ok_or_else(|| {
        format!("invalid recording region '{value}'; width and height must be positive")
    })
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

fn parse_capture_target(value: &str) -> Result<CaptureTarget, String> {
    match value {
        "screen" | "full" | "monitor" => Ok(CaptureTarget::Screen),
        "region" | "area" => Ok(CaptureTarget::Region),
        "window" => Ok(CaptureTarget::Window),
        other => Err(format!(
            "unknown capture target '{other}'; expected screen, region, or window"
        )),
    }
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
            "--diagnostics-output" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--diagnostics-output requires a file path".to_string())?;
                cli.diagnostics_output = Some(PathBuf::from(value));
            }
            "--record-selftest" => cli.record_selftest = true,
            "--record-no-cursor" => cli.record_no_cursor = true,
            "--record" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--record requires a value: screen or region".to_string())?;
                cli.record = Some(parse_capture_target(&value)?);
            }
            "--record-duration" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--record-duration requires a number of seconds".to_string())?;
                let seconds: u32 = value.parse().map_err(|_| {
                    format!("invalid recording duration '{value}'; expected whole seconds")
                })?;
                if seconds > 86_400 {
                    return Err(format!(
                        "recording duration {seconds} is out of range; expected 0 to 86400 seconds"
                    ));
                }
                cli.record_duration = seconds;
            }
            "--record-fps" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--record-fps requires a frame rate".to_string())?;
                let fps: u32 = value.parse().map_err(|_| {
                    format!("invalid frame rate '{value}'; expected a whole number")
                })?;
                if !(snipunix_record::MIN_FPS..=snipunix_record::MAX_FPS).contains(&fps) {
                    return Err(format!(
                        "frame rate {fps} is out of range; expected {} to {}",
                        snipunix_record::MIN_FPS,
                        snipunix_record::MAX_FPS
                    ));
                }
                cli.record_fps = Some(fps);
            }
            "--record-output" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--record-output requires a file path".to_string())?;
                cli.record_output = Some(PathBuf::from(value));
            }
            "--record-region" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--record-region requires X,Y,W,H".to_string())?;
                cli.record_region = Some(parse_region(&value)?);
            }
            "--record-audio" => {
                let value = args.next().ok_or_else(|| {
                    "--record-audio requires none, mic, system, or both".to_string()
                })?;
                cli.record_audio = Some(RecordAudio::parse(&value).ok_or_else(|| {
                    format!("unknown audio source '{value}'; expected none, mic, system, or both")
                })?);
            }
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
                cli.target = Some(parse_capture_target(&value)?);
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
    --diagnostics-output <PATH> Write a diagnostics report to PATH and exit.
    --include-cursor            Draw the pointer into the capture (X11).
    --record <screen|region>
                                Record instead of taking a screenshot.
    --record-output <PATH>      Recording file; defaults to the recording folder.
    --record-duration <SECONDS> Stop after SECONDS; 0 records until Ctrl+C.
    --record-fps <N>            Recording frame rate, 10–60 (default 30).
    --record-region <X,Y,W,H>   Region in logical pixels inside the recorded monitor.
    --record-audio <none|mic|system|both>
                                Audio sources (default from settings).
    --record-no-cursor          Leave the pointer out of the recording.
    --record-selftest           Encode a synthetic pattern and report the encoder.
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
    snipunix --record screen --record-duration 10 --record-output /tmp/demo.mp4
    snipunix --record region --record-region 100,100,1280,720 --record-duration 30
    snipunix --record-selftest
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
    fn parses_recording_options() {
        let ParseResult::Run(cli) = parse_args(&[
            "--record",
            "region",
            "--record-output",
            "/tmp/demo.mp4",
            "--record-duration",
            "30",
            "--record-fps",
            "25",
            "--record-region",
            "10,20,800,600",
            "--record-audio",
            "both",
            "--record-no-cursor",
        ])
        .unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.record, Some(CaptureTarget::Region));
        assert_eq!(cli.record_output, Some(PathBuf::from("/tmp/demo.mp4")));
        assert_eq!(cli.record_duration, 30);
        assert_eq!(cli.record_fps, Some(25));
        assert_eq!(cli.record_region, LogicalCrop::new(10, 20, 800, 600));
        assert_eq!(cli.record_audio, Some(RecordAudio::Both));
        assert!(cli.record_no_cursor);
    }

    #[test]
    fn recording_defaults_are_zero_and_unset() {
        let ParseResult::Run(cli) = parse_args(&["--record", "screen"]).unwrap() else {
            panic!("expected run");
        };
        assert_eq!(cli.record, Some(CaptureTarget::Screen));
        assert_eq!(cli.record_duration, 0);
        assert_eq!(cli.record_fps, None);
        assert_eq!(cli.record_audio, None);
        assert!(!cli.record_selftest);
    }

    #[test]
    fn rejects_invalid_recording_values() {
        assert!(parse_args(&["--record"]).is_err());
        assert!(parse_args(&["--record", "desktop"]).is_err());
        assert!(parse_args(&["--record-fps", "5"]).is_err());
        assert!(parse_args(&["--record-fps", "120"]).is_err());
        assert!(parse_args(&["--record-fps", "abc"]).is_err());
        assert!(parse_args(&["--record-duration", "999999"]).is_err());
        assert!(parse_args(&["--record-region", "1,2,3"]).is_err());
        assert!(parse_args(&["--record-region", "1,2,0,4"]).is_err());
        assert!(parse_args(&["--record-region", "1,2,x,4"]).is_err());
        assert!(parse_args(&["--record-audio", "loud"]).is_err());
    }

    #[test]
    fn audio_choices_map_to_engine_options() {
        assert!(!RecordAudio::None.options().any());
        assert!(RecordAudio::Microphone.options().microphone);
        assert!(RecordAudio::System.options().system);
        assert!(RecordAudio::Both.options().any());
        assert_eq!(RecordAudio::parse("monitor"), Some(RecordAudio::System));
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parse_args(&["--help"]).unwrap(), ParseResult::Help);
        assert_eq!(parse_args(&["-V"]).unwrap(), ParseResult::Version);
    }
}
