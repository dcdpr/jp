//! The programs `cut`, `pick`, and `finalize` run: git, and the project's
//! `bump`.
//!
//! Each runs in a directory it is given, reads nothing from stdin, and has its
//! output captured, since the plugin's own stdin and stdout carry the host's
//! protocol.
//! [`Shell`] holds the environment they run with on top of the plugin's, which
//! a test pins so that the machine's git configuration can't reach in.

use std::{
    ffi::OsStr,
    process::{Command, Stdio},
};

use camino::Utf8Path;

/// Runs programs for the release pipeline.
#[derive(Debug, Clone, Default)]
pub(crate) struct Shell {
    /// Set on every program, on top of the plugin's own environment.
    env: Vec<(String, String)>,
}

/// What a program printed, and whether it succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ran {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    /// What the program printed to explain itself: stderr, or stdout when
    /// stderr is empty.
    pub(crate) fn said(&self) -> &str {
        let stderr = self.stderr.trim();
        if stderr.is_empty() {
            self.stdout.trim()
        } else {
            stderr
        }
    }
}

impl Shell {
    /// A shell that runs programs with the plugin's environment.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A shell that sets `env` on every program it runs.
    #[cfg(test)]
    pub(crate) fn with_env(env: Vec<(String, String)>) -> Self {
        Self { env }
    }

    /// Run `program` with `args` in `dir`, whatever it exits with.
    ///
    /// The error says why it couldn't be run at all.
    pub(crate) fn run<S: AsRef<OsStr>>(
        &self,
        dir: &Utf8Path,
        program: &str,
        args: &[S],
    ) -> Result<Ran, String> {
        let output = Command::new(program)
            .args(args)
            .current_dir(dir)
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("`{program}` couldn't be run: {error}"))?;

        Ok(Ran {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Run git with `args` in `dir`, whatever it exits with.
    pub(crate) fn try_git(&self, dir: &Utf8Path, args: &[&str]) -> Result<Ran, String> {
        self.run(dir, "git", args)
    }

    /// Run git with `args` in `dir`, and return what it printed on stdout.
    ///
    /// The error names the command and quotes what git said.
    pub(crate) fn git(&self, dir: &Utf8Path, args: &[&str]) -> Result<String, String> {
        let ran = self.try_git(dir, args)?;
        if !ran.success {
            return Err(format!("`git {}` failed: {}", subcommand(args), ran.said()));
        }

        Ok(ran.stdout)
    }
}

/// The subcommand `args` run, such as `push`: the first argument that is
/// neither an option nor a `-c` setting.
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    args.iter()
        .copied()
        .find(|arg| !arg.starts_with('-') && !arg.contains('='))
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
