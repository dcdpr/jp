//! `qual`: compare rustqual findings between revisions, finding by finding.
//!
//! Exits 0 when the comparison passes, 1 when it finds something, and 2 when it
//! can't be made.

use std::{
    env,
    error::Error,
    io::{self, Write as _},
    process::ExitCode,
};

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use jp_process::{ProcessRunner, SystemProcessRunner};
use qual::{Format, Workspace, render};

#[derive(Debug, Parser)]
#[command(
    name = "qual",
    about = "Compare rustqual findings between revisions, finding by finding."
)]
struct Cli {
    /// The root of the checkout.
    /// Defaults to the one containing the current directory.
    #[arg(long, global = true)]
    root: Option<Utf8PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print what the working tree adds since it forked from a base revision,
    /// and every finding of an enforced rule.
    ///
    /// Exits 1 when there is any.
    Diff {
        /// The revision to compare against.
        /// The working tree is compared with its merge base with this revision,
        /// so commits made on it after the fork don't count.
        #[arg(long)]
        base: String,

        #[arg(long, value_enum, default_value_t)]
        format: Format,
    },

    /// Print every finding in the working tree, leaving out exempt rules.
    Report {
        #[arg(long, value_enum, default_value_t)]
        format: Format,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            drop(writeln!(io::stderr(), "qual: {error}"));
            ExitCode::from(2)
        }
    }
}

/// Run the command, returning whether it passed.
fn run(cli: Cli) -> Result<bool, Box<dyn Error>> {
    let runner = SystemProcessRunner;
    let root = match cli.root {
        Some(root) => root,
        None => toplevel(&runner)?,
    };
    let workspace = Workspace::new(root);

    let (out, passed) = match cli.command {
        Command::Diff { base, format } => {
            let verdict = workspace.diff(&runner, &base)?;
            let guidance = workspace.guidance(verdict.rules())?;
            (
                render::verdict(&verdict, &guidance, format),
                verdict.passed(),
            )
        }
        Command::Report { format } => {
            let report = workspace.report(&runner)?;
            let rules = report.findings.iter().map(|finding| finding.rule.as_str());
            let guidance = workspace.guidance(rules)?;
            (render::report(&report, &guidance, format), true)
        }
    };

    io::stdout().write_all(out.as_bytes())?;
    Ok(passed)
}

/// The root of the git checkout containing the current directory.
fn toplevel(runner: &dyn ProcessRunner) -> Result<Utf8PathBuf, Box<dyn Error>> {
    let cwd = Utf8PathBuf::try_from(env::current_dir()?)?;
    let output = runner.run("git", &["rev-parse", "--show-toplevel"], &cwd)?;
    if !output.success() {
        return Err(format!("not inside a git checkout: {}", output.stderr.trim()).into());
    }
    Ok(Utf8PathBuf::from(output.stdout.trim()))
}
