//! Command handling for the Rust foundation.
//!
//! The live dashboard owns collection only for the lifetime of its terminal pane.

use std::path::PathBuf;

pub mod app_state;
pub mod collector;
pub mod domain;
pub mod sanitize;
pub mod scheduler;
pub mod sidebar;
pub mod store;
pub mod ui;
mod unix_signal;

/// The commands that this foundation supports.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Print the package version.
    Version,
    /// Render a sanitized fixture as deterministic terminal text.
    Preview {
        fixture: PathBuf,
        width: u16,
        height: u16,
        svg: Option<PathBuf>,
    },
    /// Open the live, pane-owned terminal dashboard.
    Dashboard,
    /// Toggle the pane-preserving Herdr sidebar.
    Sidebar,
}

/// An error from command-line parsing.
#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    message: String,
}

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse the supported command line.
pub fn parse_command<I, S>(arguments: I) -> Result<Command, ParseError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let arguments = arguments.into_iter().map(Into::into).collect::<Vec<_>>();

    match arguments.as_slice() {
        [flag] if flag == "--version" => Ok(Command::Version),
        [command, flag, fixture, rest @ ..] if command == "preview" && flag == "--fixture" => {
            parse_preview(PathBuf::from(fixture), rest)
        }
        [command] if command == "dashboard" => Ok(Command::Dashboard),
        [command] if command == "sidebar" => Ok(Command::Sidebar),
        _ => Err(ParseError::new(usage())),
    }
}

/// Return command help for unsupported input.
pub fn usage() -> &'static str {
    "usage: herdr-quota --version | herdr-quota preview --fixture <path> [--width <cells> --height <rows> --svg <path>] | herdr-quota dashboard | herdr-quota sidebar"
}

/// Return the package version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parse_preview(fixture: PathBuf, rest: &[String]) -> Result<Command, ParseError> {
    let mut width = 36;
    let mut height = 23;
    let mut svg = None;
    let mut arguments = rest.iter();
    while let Some(flag) = arguments.next() {
        let value = arguments.next().ok_or_else(|| ParseError::new(usage()))?;
        match flag.as_str() {
            "--width" => width = value.parse().map_err(|_| ParseError::new(usage()))?,
            "--height" => height = value.parse().map_err(|_| ParseError::new(usage()))?,
            "--svg" => svg = Some(PathBuf::from(value)),
            _ => return Err(ParseError::new(usage())),
        }
    }
    if width == 0 || height == 0 {
        return Err(ParseError::new(usage()));
    }
    Ok(Command::Preview {
        fixture,
        width,
        height,
        svg,
    })
}

#[cfg(test)]
mod tests {
    use super::{Command, parse_command, usage, version};
    use std::path::PathBuf;

    #[test]
    fn parses_version() {
        assert_eq!(parse_command(["--version"]), Ok(Command::Version));
    }

    #[test]
    fn parses_fixture_only_preview() {
        assert_eq!(
            parse_command(["preview", "--fixture", "test/fixture.json"]),
            Ok(Command::Preview {
                fixture: PathBuf::from("test/fixture.json"),
                width: 36,
                height: 23,
                svg: None,
            })
        );
    }

    #[test]
    fn rejects_unsupported_input() {
        assert_eq!(
            parse_command(["preview"]),
            Err(super::ParseError::new(usage()))
        );
    }

    #[test]
    fn reports_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn parses_preview_dimensions_and_svg() {
        assert_eq!(
            parse_command([
                "preview",
                "--fixture",
                "test/fixture.json",
                "--width",
                "20",
                "--height",
                "12",
                "--svg",
                "docs/preview.svg"
            ]),
            Ok(Command::Preview {
                fixture: PathBuf::from("test/fixture.json"),
                width: 20,
                height: 12,
                svg: Some(PathBuf::from("docs/preview.svg")),
            })
        );
    }

    #[test]
    fn parses_only_the_live_dashboard_entrypoint() {
        assert_eq!(parse_command(["dashboard"]), Ok(Command::Dashboard));
        assert_eq!(
            parse_command(["dashboard", "--fixture", "test/fixture.json"]),
            Err(super::ParseError::new(usage()))
        );
    }

    #[test]
    fn parses_the_sidebar_action_entrypoint() {
        assert_eq!(parse_command(["sidebar"]), Ok(Command::Sidebar));
        assert_eq!(
            parse_command(["sidebar", "--focus"]),
            Err(super::ParseError::new(usage()))
        );
    }
}
