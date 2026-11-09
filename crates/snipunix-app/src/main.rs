#![forbid(unsafe_code)]

mod app;
mod cli;
mod editor;
mod shortcuts;

use std::process::ExitCode;

use anyhow::{Context, Result};
use snipunix_capture::{CaptureBackend, CaptureOptions, PortalScreenshot};
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

            // Headless scripting path: full-screen capture straight to a file,
            // without initialising GTK.
            if let (Some(cli::CaptureTarget::Screen), Some(output)) =
                (cli.target, cli.output.as_ref())
            {
                capture_screen_to_file(output)?;
                return Ok(ExitCode::SUCCESS);
            }

            Ok(ExitCode::from(app::run(cli).get()))
        }
    }
}

fn capture_screen_to_file(output: &std::path::Path) -> Result<()> {
    let backend = PortalScreenshot::default();
    let image = async_io::block_on(backend.capture(CaptureOptions::default()))
        .map_err(|error| anyhow::anyhow!(error))
        .context("failed to capture the screen through the desktop portal")?;
    image
        .save_png(output)
        .with_context(|| format!("failed to write {}", output.display()))?;
    println!(
        "Wrote {}x{} PNG to {}",
        image.width(),
        image.height(),
        output.display()
    );
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
