//! CLI for comparing `rustqual` findings across git revisions.
//!
//! Exits 0 when the check passes, 1 when new or enforced findings are reported,
//! and 2 on execution or parse errors.

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
    /// Git working tree root.
    /// Defaults to the repository containing the current directory.
    #[arg(long, global = true)]
    root: Option<Utf8PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Report new findings since the merge base with `--base`, plus any
    /// findings for enforced rules.
    ///
    /// Exits with code 1 if any new or enforced findings are present.
    Diff {
        /// Base revision to compare against (at `git merge-base HEAD <base>`).
        ///
        /// Passing git's null object ID (40 or 64 zeros) skips the base
        /// comparison and reports only findings for enforced rules.
        #[arg(long)]
        base: String,

        #[arg(long, value_enum, default_value_t)]
        format: Format,
    },

    /// List all non-exempt findings in the working tree.
    Report {
        /// List the findings of this rule only.
        ///
        /// Fails when the policy exempts the rule.
        #[arg(long)]
        rule: Option<String>,

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
        Command::Report { rule, format } => {
            let report = match &rule {
                Some(rule) => workspace.report_rule(&runner, rule)?,
                None => workspace.report(&runner)?,
            };
            let rules = report.findings.iter().map(|finding| finding.rule.as_str());
            let guidance = workspace.guidance(rules)?;
            (render::report(&report, &guidance, format), true)
        }
    };

    io::stdout().write_all(out.as_bytes())?;
    Ok(passed)
}

fn toplevel(runner: &dyn ProcessRunner) -> Result<Utf8PathBuf, Box<dyn Error>> {
    let cwd = Utf8PathBuf::try_from(env::current_dir()?)?;
    let output = runner.run("git", &["rev-parse", "--show-toplevel"], &cwd)?;
    if !output.success() {
        return Err(format!("not inside a git checkout: {}", output.stderr.trim()).into());
    }
    Ok(Utf8PathBuf::from(output.stdout.trim()))
}
