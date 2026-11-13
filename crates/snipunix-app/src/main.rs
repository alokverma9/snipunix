#![forbid(unsafe_code)]

mod app;
mod cli;
mod editor;
mod onboarding;
mod settings_ui;
mod shortcuts;
mod tray;

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use snipunix_capture::{
    AutoCapture, CaptureBackend, CaptureOptions, ScreencastOptions, screencast,
};
use snipunix_record::{AudioOptions, PipeWireStream, RecordOptions, Recorder, VideoSource};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    init_logging();
    match real_main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("snipunix: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match cli::parse(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("snipunix: {error}\n\n{}", cli::HELP);
            return Ok(ExitCode::from(2));
        }
    };

    match parsed {
        cli::ParseResult::Help => {
            print!("{}", cli::HELP);
            Ok(ExitCode::SUCCESS)
        }
        cli::ParseResult::Version => {
            println!("snipunix {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        cli::ParseResult::Run(cli) => {
            if cli.portal_info {
                print_portal_info()?;
                return Ok(ExitCode::SUCCESS);
            }
            if cli.download_ocr_models {
                download_ocr_models(cli.ocr_models.as_deref())?;
                return Ok(ExitCode::SUCCESS);
            }
            if let Some(path) = cli.ocr.as_deref() {
                run_ocr(path, cli.ocr_models.as_deref(), cli.remove_line_breaks)?;
                return Ok(ExitCode::SUCCESS);
            }
            if cli.list_windows {
                list_windows()?;
                return Ok(ExitCode::SUCCESS);
            }
            if cli.diagnostics {
                print_diagnostics(&cli)?;
                return Ok(ExitCode::SUCCESS);
            }
            if cli.record_selftest {
                run_record_selftest()?;
                return Ok(ExitCode::SUCCESS);
            }

            // Recording has its own headless path and does not initialise GTK.
            if let Some(target) = cli.record {
                let output = cli
                    .record_output
                    .clone()
                    .or_else(|| cli.output.clone())
                    .unwrap_or_else(default_recording_path);
                record_to_file(&cli, target, &output)?;
                return Ok(ExitCode::SUCCESS);
            }

            // Headless scripting path: screen or window capture straight to a
            // file, without initialising GTK.
            if let (Some(target), Some(output)) = (cli.target, cli.output.as_ref()) {
                match target {
                    cli::CaptureTarget::Region => {}
                    cli::CaptureTarget::Screen | cli::CaptureTarget::Window => {
                        capture_to_file(&cli, target, output)?;
                        return Ok(ExitCode::SUCCESS);
                    }
                }
            }

            Ok(ExitCode::from(app::run(cli).get()))
        }
    }
}

fn capture_to_file(
    cli: &cli::Cli,
    target: cli::CaptureTarget,
    output: &std::path::Path,
) -> Result<()> {
    let window = match target {
        cli::CaptureTarget::Window => Some(cli.window.ok_or_else(|| {
            anyhow::anyhow!("--capture window requires --window <ID> in the headless path")
        })?),
        _ => None,
    };
    let backend = AutoCapture::new(cli.backend);
    let frame = async_io::block_on(backend.capture(CaptureOptions {
        interactive: false,
        parent: None,
        window,
        include_cursor: cli.include_cursor,
    }))
    .map_err(|error| anyhow::anyhow!(error))
    .context("failed to capture the screen")?;
    frame
        .image
        .save_png(output)
        .with_context(|| format!("failed to write {}", output.display()))?;
    println!(
        "Wrote {}x{} PNG to {} ({} monitor{})",
        frame.image.width(),
        frame.image.height(),
        output.display(),
        frame.monitors.len(),
        if frame.monitors.len() == 1 { "" } else { "s" },
    );
    Ok(())
}

fn default_recording_path() -> PathBuf {
    let settings = snipunix_settings::Settings::load(&snipunix_settings::Settings::default_path())
        .unwrap_or_default();
    let stamp = glib::DateTime::now_local()
        .ok()
        .and_then(|date| date.format("%Y-%m-%d_%H-%M-%S").ok())
        .map_or_else(|| "recording".to_string(), |value| value.to_string());
    settings
        .recording_directory()
        .join(format!("Recording_{stamp}.mp4"))
}

fn record_to_file(cli: &cli::Cli, target: cli::CaptureTarget, output: &Path) -> Result<()> {
    if target == cli::CaptureTarget::Window {
        anyhow::bail!("--record window is not supported; use screen or region");
    }
    let settings = snipunix_settings::Settings::load(&snipunix_settings::Settings::default_path())
        .unwrap_or_default();
    let crop = if target == cli::CaptureTarget::Region {
        Some(cli.record_region.ok_or_else(|| {
            anyhow::anyhow!("--record region requires --record-region X,Y,W,H in the headless path")
        })?)
    } else {
        None
    };

    let include_cursor = !cli.record_no_cursor && settings.recording.include_cursor;
    let audio = cli
        .record_audio
        .map(cli::RecordAudio::options)
        .unwrap_or(AudioOptions {
            microphone: settings.recording.microphone,
            system: settings.recording.system_audio,
        });
    let fps = cli.record_fps.unwrap_or(settings.recording.fps);

    async_io::block_on(screencast::register_host_identity(app::APP_ID));
    let session = async_io::block_on(screencast::open(
        ScreencastOptions::recording(include_cursor, settings.recording.restore_token.clone()),
        screencast::DEFAULT_SCREENCAST_TIMEOUT,
    ))
    .map_err(|error| anyhow::anyhow!(error))
    .context("could not start a ScreenCast session")?;

    let stream = session
        .primary_stream()
        .ok_or_else(|| anyhow::anyhow!("the ScreenCast portal returned no stream"))?;
    let source = VideoSource::PipeWire(PipeWireStream {
        fd: session.pipewire_fd().as_raw_fd(),
        node_id: stream.node_id,
        serial: stream.serial,
        logical_size: stream.logical_size,
    });

    let mut recorder = Recorder::start(
        source,
        RecordOptions {
            output: output.to_path_buf(),
            fps,
            bitrate_kbps: settings.recording.bitrate_kbps,
            crop,
            audio,
        },
    )
    .map_err(|error| anyhow::anyhow!(error))
    .context("could not start recording")?;

    if let Some(token) = session.restore_token() {
        if settings.recording.restore_token.as_deref() != Some(token) {
            let mut updated = settings.clone();
            updated.recording.restore_token = Some(token.to_string());
            if let Err(error) = updated.save(&snipunix_settings::Settings::default_path()) {
                tracing::warn!(%error, "could not persist the ScreenCast restore token");
            }
        }
    }

    let (width, height) = recorder.dimensions();
    if cli.record_duration == 0 {
        eprintln!(
            "Recording {width}x{height} at {fps} fps with {}. Press Ctrl+C to stop.",
            recorder.encoder().label()
        );
    } else {
        eprintln!(
            "Recording {width}x{height} at {fps} fps with {} for {} s.",
            recorder.encoder().label(),
            cli.record_duration
        );
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    ctrlc::set_handler(move || stop_flag.store(true, Ordering::SeqCst))
        .context("could not install the stop handler")?;

    let limit = Duration::from_secs(u64::from(cli.record_duration));
    loop {
        if stop.load(Ordering::SeqCst) || (cli.record_duration > 0 && recorder.elapsed() >= limit) {
            break;
        }
        if let Some(error) = recorder.take_error() {
            recorder.cancel();
            async_io::block_on(session.close());
            return Err(anyhow::anyhow!(error)).context("recording failed");
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let file = match recorder.stop() {
        Ok(file) => file,
        Err(error) => {
            async_io::block_on(session.close());
            return Err(anyhow::anyhow!(error)).context("could not finalise the recording");
        }
    };
    async_io::block_on(session.close());

    println!(
        "Wrote {} ({:.1} s, {} bytes, {}x{}, {}{})",
        file.path.display(),
        file.duration.as_secs_f64(),
        file.bytes,
        file.width,
        file.height,
        file.encoder.label(),
        if file.has_audio { ", audio" } else { "" },
    );
    Ok(())
}

fn run_record_selftest() -> Result<()> {
    snipunix_record::init().map_err(|error| anyhow::anyhow!(error))?;
    let support = snipunix_record::encoder_support();
    println!("GStreamer: {}", snipunix_record::version());
    if support.available.is_empty() {
        anyhow::bail!(
            "no H.264 encoder is installed; install gstreamer1.0-plugins-ugly or \
             gstreamer1.0-vaapi"
        );
    }
    println!(
        "Encoders: {}",
        support
            .available
            .iter()
            .map(|encoder| encoder.label())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let requested_output = std::env::var_os("SNIPUNIX_SELFTEST_OUTPUT").map(PathBuf::from);
    let output = requested_output.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "snipunix-record-selftest-{}.mp4",
            std::process::id()
        ))
    });
    let started = Instant::now();
    let recorder = Recorder::start(
        VideoSource::TestPattern {
            width: 640,
            height: 360,
        },
        RecordOptions {
            fps: 30,
            bitrate_kbps: 2000,
            ..RecordOptions::new(&output)
        },
    )
    .map_err(|error| anyhow::anyhow!(error))
    .context("recording self-test failed to start")?;
    std::thread::sleep(Duration::from_secs(2));
    let file = recorder
        .stop()
        .map_err(|error| anyhow::anyhow!(error))
        .context("recording self-test failed to finalise")?;
    println!(
        "Self-test wrote {} bytes ({}x{}, {:?}, {}) in {:.2} s",
        file.bytes,
        file.width,
        file.height,
        file.encoder,
        file.path.display(),
        started.elapsed().as_secs_f64()
    );
    if requested_output.is_none() {
        std::fs::remove_file(&output).ok();
    }
    Ok(())
}

/// Builds a diagnostics report from environment and backend state. It never
/// reads screenshots, OCR text, clipboard content, or user documents.
fn print_diagnostics(cli: &cli::Cli) -> Result<()> {
    let mut out = String::new();
    out.push_str("snipunix diagnostics\n");
    out.push_str(&format!("version: {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!(
        "platform: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    for key in [
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "GDK_BACKEND",
        "GDK_SCALE",
    ] {
        if let Ok(value) = std::env::var(key) {
            out.push_str(&format!("{key}: {value}\n"));
        }
    }
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") if std::env::var_os("WAYLAND_DISPLAY").is_some() => {
            out.push_str("session: Wayland desktop\n");
        }
        Ok("wayland") => out.push_str(
            "warning: XDG_SESSION_TYPE=wayland but WAYLAND_DISPLAY is unset; \
             the desktop Wayland socket is unavailable and DISPLAY may be a \
             test or nested server\n",
        ),
        Ok("x11") => out.push_str("session: X11 desktop\n"),
        Ok(other) if !other.is_empty() => out.push_str(&format!("session: {other}\n")),
        _ => {}
    }
    if let Ok(release) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        out.push_str(&format!("kernel: {}\n", release.trim()));
    }
    if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
        for line in os_release.lines() {
            if line.starts_with("PRETTY_NAME=") {
                out.push_str(&format!(
                    "distribution: {}\n",
                    line.trim_start_matches("PRETTY_NAME=").trim_matches('"')
                ));
            }
        }
    }
    out.push_str(&format!(
        "auto backend: {}\n",
        snipunix_capture::detected_backend().label()
    ));
    out.push_str(&format!("requested backend: {}\n", cli.backend.label()));

    match async_io::block_on(snipunix_capture::probe_portal()) {
        Ok(info) => {
            out.push_str(&format!("screenshot portal: version {}\n", info.version));
            match info.available_targets {
                Some(bits) => out.push_str(&format!("screenshot targets: {bits:#x}\n")),
                None => out.push_str("screenshot targets: unknown (v2 or older)\n"),
            }
        }
        Err(error) => out.push_str(&format!("screenshot portal: unavailable ({error})\n")),
    }

    match async_io::block_on(snipunix_capture::probe_screencast()) {
        Ok(info) => {
            out.push_str(&format!("screencast portal: version {}\n", info.version));
            out.push_str(&format!(
                "screencast sources: {:#x}\n",
                info.source_types.bits()
            ));
            out.push_str(&format!(
                "screencast cursor modes: {:#x}\n",
                info.cursor_modes.bits()
            ));
        }
        Err(error) => out.push_str(&format!("screencast portal: unavailable ({error})\n")),
    }

    match snipunix_record::init() {
        Ok(()) => {
            let support = snipunix_record::encoder_support();
            out.push_str(&format!("gstreamer: {}\n", snipunix_record::version()));
            let encoders = support
                .available
                .iter()
                .map(|encoder| encoder.label())
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "gstreamer encoders: {}\n",
                if encoders.is_empty() {
                    "none"
                } else {
                    &encoders
                }
            ));
            match support.preferred {
                Some(encoder) => out.push_str(&format!(
                    "gstreamer preferred encoder: {}{}\n",
                    encoder.label(),
                    if encoder.is_hardware() {
                        " (hardware)"
                    } else {
                        ""
                    }
                )),
                None => out.push_str("gstreamer preferred encoder: none\n"),
            }
        }
        Err(error) => out.push_str(&format!("gstreamer: unavailable ({error})\n")),
    }

    if std::env::var_os("DISPLAY").is_some() {
        match snipunix_capture::X11Capture::new().diagnostics() {
            Ok(report) => out.push_str(&report),
            Err(error) => out.push_str(&format!("X11: unavailable ({error})\n")),
        }
    } else {
        out.push_str("X11: no DISPLAY\n");
    }

    let settings_path = snipunix_settings::Settings::default_path();
    let settings = snipunix_settings::Settings::load(&settings_path).unwrap_or_default();
    out.push_str(&format!(
        "settings: {} ({})\n",
        settings_path.display(),
        if settings_path.exists() {
            "present"
        } else {
            "defaults"
        }
    ));
    out.push_str(&format!(
        "capture: mode={:?} delay={} cursor={} copy={}\n",
        settings.capture.default_mode,
        settings.capture.delay,
        settings.capture.include_cursor,
        settings.capture.copy_after_capture,
    ));
    out.push_str(&format!(
        "recording: fps={} bitrate={}kbps cursor={} mic={} system={} restore-token={}\n",
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
    ));
    match snipunix_ocr::ModelPaths::discover(settings.ocr.model_directory.clone().as_deref()) {
        Ok(paths) => out.push_str(&format!(
            "ocr models: {} and {}\n",
            paths.detection.display(),
            paths.recognition.display()
        )),
        Err(error) => out.push_str(&format!("ocr models: missing ({error})\n")),
    }

    print!("{out}");
    Ok(())
}

fn list_windows() -> Result<()> {
    let windows = snipunix_capture::X11Capture::new()
        .list_windows()
        .map_err(|error| anyhow::anyhow!(error))
        .context("failed to list X11 windows")?;
    if windows.is_empty() {
        println!("No capturable X11 windows.");
        return Ok(());
    }
    for window in windows {
        println!(
            "{:#010x}  {:>5}x{:<5} | {:<24} | {}",
            window.id,
            window.width,
            window.height,
            if window.class.is_empty() {
                "-"
            } else {
                &window.class
            },
            window.title,
        );
    }
    Ok(())
}

fn download_ocr_models(directory: Option<&std::path::Path>) -> Result<()> {
    let directory = directory.map_or_else(
        || snipunix_settings::Settings::default().model_directory(),
        std::path::Path::to_path_buf,
    );
    let models = snipunix_ocr::download_models(&directory)
        .with_context(|| format!("failed to download OCR models to {}", directory.display()))?;
    println!("Detected model: {}", models.detection.display());
    println!("Recognition model: {}", models.recognition.display());
    Ok(())
}

fn run_ocr(
    path: &std::path::Path,
    model_directory: Option<&std::path::Path>,
    remove_line_breaks: bool,
) -> Result<()> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read image {}", path.display()))?;
    let image = snipunix_capture::decode_image(&bytes, Some(path.display().to_string()))
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("failed to decode image {}", path.display()))?;
    let models = snipunix_ocr::ModelPaths::discover(model_directory)
        .map_err(|error| anyhow::anyhow!(error))?;
    let result = snipunix_ocr::recognize(image.rgba(), image.width(), image.height(), &models)
        .map_err(|error| anyhow::anyhow!(error))
        .context("OCR failed")?;
    println!("{}", result.text(remove_line_breaks));
    Ok(())
}

fn print_portal_info() -> Result<()> {
    let info = async_io::block_on(snipunix_capture::probe_portal())
        .map_err(|error| anyhow::anyhow!(error))
        .context("failed to query the screenshot portal")?;
    println!(
        "org.freedesktop.portal.Screenshot version: {}",
        info.version
    );
    match info.available_targets {
        Some(bits) => println!("AvailableTargets: {bits:#x}"),
        None => println!("AvailableTargets: not available (Screenshot v2 backend)"),
    }
    Ok(())
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("snipunix=info,ashpd=warn,zbus=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
