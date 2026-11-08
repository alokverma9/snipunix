//! Minimal command-line parser.
//!
//! M0 deliberately avoids a parsing crate: the surface is small and keeping the
//! dependency list short matters more than convenience here.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    Screen,
    Region,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    pub target: Option<CaptureTarget>,
    pub output: Option<PathBuf>,
    pub exit_after: bool,
    pub register_shortcuts: bool,
    pub portal_info: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseResult {
    Run(Cli),
    Help,
    Version,
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
            "--capture" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--capture requires a value: screen or region".to_string())?;
                cli.target = Some(match value.as_str() {
                    "screen" | "full" | "monitor" => CaptureTarget::Screen,
                    "region" | "area" => CaptureTarget::Region,
                    other => {
                        return Err(format!(
                            "unknown capture target '{other}'; expected screen or region"
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
    --capture <screen|region>   Capture immediately after startup.
    -o, --output <PATH>         Write the resulting PNG to PATH.
    --exit                      Quit after the capture completes.
    --register-shortcuts        Register global shortcuts through the portal.
    --portal-info               Print Screenshot portal capabilities and exit.
    -h, --help                  Show this help.
    -V, --version               Show the version.

EXAMPLES:
    snipunix
    snipunix --capture screen --output /tmp/snipunix-screen.png
    snipunix --capture region --output /tmp/snipunix-region.png
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
        assert!(parse_args(&["--capture", "window"]).is_err());
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parse_args(&["--help"]).unwrap(), ParseResult::Help);
        assert_eq!(parse_args(&["-V"]).unwrap(), ParseResult::Version);
    }
}
