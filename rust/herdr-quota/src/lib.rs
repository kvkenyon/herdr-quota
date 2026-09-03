//! Command handling for the Rust foundation.
//!
//! This package does not collect quota data or start the Herdr runtime.

use std::path::PathBuf;

pub mod collector;
pub mod domain;
pub mod sanitize;
pub mod store;
pub mod ui;
mod unix_signal;

/// The commands that this foundation supports.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Print the package version.
    Version,
    /// Reserve a fixture input for a later preview implementation.
    Preview { fixture: PathBuf },
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
        [command, flag, fixture] if command == "preview" && flag == "--fixture" => {
            Ok(Command::Preview {
                fixture: PathBuf::from(fixture),
            })
        }
        _ => Err(ParseError::new(usage())),
    }
}

/// Return command help for unsupported input.
pub fn usage() -> &'static str {
    "usage: herdr-quota --version | herdr-quota preview --fixture <path>"
}

/// Return the package version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Return the placeholder text for the fixture-only preview command.
pub fn preview_message(fixture: &std::path::Path) -> String {
    format!(
        "preview placeholder: fixture '{}' is not rendered yet",
        fixture.display()
    )
}

#[cfg(test)]
mod tests {
    use super::{Command, parse_command, preview_message, usage, version};
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
    fn labels_preview_as_a_placeholder() {
        assert_eq!(
            preview_message(std::path::Path::new("test/fixture.json")),
            "preview placeholder: fixture 'test/fixture.json' is not rendered yet"
        );
    }
}
