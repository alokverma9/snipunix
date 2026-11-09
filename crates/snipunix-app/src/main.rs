#![forbid(unsafe_code)]

mod app;
mod cli;
mod editor;
mod onboarding;
mod settings_ui;
mod shortcuts;
mod tray;

use std::process::ExitCode;

use anyhow::{Context, Result};
use snipunix_capture::{AutoCapture, CaptureBackend, CaptureOptions};
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
