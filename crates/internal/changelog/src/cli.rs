//! The `changelog` binary.
//!
//! `changelog check [DIR]` holds a fragments directory, `changelog.d` unless
//! `DIR` names another, to its layout and every fragment in it to the format,
//! and exits 1 when anything breaks a rule.
//! It reads no configuration, so CI can run it without `jp`.

use std::process::ExitCode;

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};

use crate::directory;

#[derive(Debug, Parser)]
#[command(
    name = "changelog",
    about = "Check the changelog fragments a project records for its users.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check every file in a fragments directory, released fragments included.
    ///
    /// Prints each problem, then how many fragments were checked.
    /// Exits 1 when anything breaks a rule, and 2 when a file or directory
    /// can't be read.
    Check {
        /// The fragments directory.
        #[arg(default_value = "changelog.d")]
        dir: Utf8PathBuf,
    },
}

/// Run the binary with the process's arguments.
#[must_use]
// The binary's output: the report on stdout, a failure to read on stderr.
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn main() -> ExitCode {
    let Cli {
        command: Command::Check { dir },
    } = Cli::parse();

    match directory::check(&dir) {
        Ok(report) => {
            println!("{report}");
            ExitCode::from(u8::from(!report.problems.is_empty()))
        }
        Err(error) => {
            eprintln!("changelog: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
