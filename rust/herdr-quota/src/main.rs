use std::process::ExitCode;

use herdr_quota::{Command, parse_command, preview_message, version};

fn main() -> ExitCode {
    match parse_command(std::env::args().skip(1)) {
        Ok(Command::Version) => {
            println!("herdr-quota {}", version());
            ExitCode::SUCCESS
        }
        Ok(Command::Preview { fixture }) => {
            println!("{}", preview_message(&fixture));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
