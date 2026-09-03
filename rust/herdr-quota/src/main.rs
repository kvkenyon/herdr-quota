use std::process::ExitCode;

use std::fs;

use herdr_quota::domain::schema::parse_quota_response;
use herdr_quota::ui::render::{DashboardConfig, dashboard, preview_svg, render_lines};
use herdr_quota::{Command, parse_command, version};

fn main() -> ExitCode {
    match parse_command(std::env::args().skip(1)) {
        Ok(Command::Version) => {
            println!("herdr-quota {}", version());
            ExitCode::SUCCESS
        }
        Ok(Command::Preview {
            fixture,
            width,
            height,
            svg,
        }) => match load_report(&fixture) {
            Ok(report) => {
                let config = DashboardConfig {
                    color: std::env::var_os("NO_COLOR").is_none(),
                    ..DashboardConfig::default()
                };
                let lines = render_lines(&report, width, height, &config);
                println!("{}", lines.join("\n"));
                if let Some(path) = svg {
                    if fs::write(path, preview_svg(&lines, width, height)).is_err() {
                        eprintln!("could not write preview SVG");
                        return ExitCode::from(1);
                    }
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Ok(Command::Dashboard { fixture }) => match load_report(&fixture) {
            Ok(report) => match dashboard(&report) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => {
                    eprintln!("dashboard requires an interactive terminal");
                    ExitCode::from(1)
                }
            },
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn load_report(
    path: &std::path::Path,
) -> Result<herdr_quota::domain::schema::QuotaReport, &'static str> {
    let input = fs::read_to_string(path).map_err(|_| "could not read fixture")?;
    parse_quota_response(&input).map_err(|_| "fixture does not contain supported quota data")
}
