//! Subprocess execution for `git` and configured `bump` / `build` commands.
//!
//! Because plugin `stdin` and `stdout` carry the NDJSON plugin protocol, child
//! processes run with `stdin` closed and their output captured (or streamed
//! line by line via [`Shell::stream`] for long-running commands like `build`).
//! [`Shell`] also carries environment overrides so tests can isolate `git` from
//! host configuration.

use std::{
    ffi::OsStr,
    io::{BufRead as _, BufReader, Read},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
};

use camino::Utf8Path;

/// Subprocess runner with optional environment variable overrides.
#[derive(Debug, Clone, Default)]
pub(crate) struct Shell {
    env: Vec<(String, String)>,
}

/// Captured output and exit status of a completed subprocess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ran {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    /// Return trimmed `stderr` if non-empty, or trimmed `stdout` otherwise.
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
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn with_env(env: Vec<(String, String)>) -> Self {
        Self { env }
    }

    /// Run `program` with `args` in `dir` and capture its output.
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

    /// Run `program` with `args` in `dir`, forwarding each stdout and stderr
    /// line (stripped of its trailing newline) to `each`.
    ///
    /// If `each` returns an error, remaining child output is still drained
    /// before waiting for the process so the child cannot block on a full pipe.
    pub(crate) fn stream<S: AsRef<OsStr>>(
        &self,
        dir: &Utf8Path,
        program: &str,
        args: &[S],
        each: &mut dyn FnMut(&str) -> Result<(), String>,
    ) -> Result<ExitStatus, String> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(dir)
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("`{program}` couldn't be run: {error}"))?;

        let (sender, lines) = mpsc::channel();
        let readers = [
            child
                .stdout
                .take()
                .map(|pipe| read_lines(pipe, sender.clone())),
            child
                .stderr
                .take()
                .map(|pipe| read_lines(pipe, sender.clone())),
        ];
        drop(sender);

        let mut failed = None;
        for line in lines {
            if failed.is_none()
                && let Err(error) = each(&line)
            {
                failed = Some(error);
            }
        }
        for reader in readers.into_iter().flatten() {
            drop(reader.join());
        }

        let status = child
            .wait()
            .map_err(|error| format!("`{program}` couldn't be waited for: {error}"))?;

        match failed {
            Some(error) => Err(error),
            None => Ok(status),
        }
    }

    pub(crate) fn try_git(&self, dir: &Utf8Path, args: &[&str]) -> Result<Ran, String> {
        self.run(dir, "git", args)
    }

    /// Run `git` with `args` in `dir`, returning `stdout` on success or an
    /// error quoting git's output on failure.
    pub(crate) fn git(&self, dir: &Utf8Path, args: &[&str]) -> Result<String, String> {
        let ran = self.try_git(dir, args)?;
        if !ran.success {
            return Err(format!("`git {}` failed: {}", subcommand(args), ran.said()));
        }

        Ok(ran.stdout)
    }
}

fn read_lines(pipe: impl Read + Send + 'static, sender: Sender<String>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut line = vec![];
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }

            let text = String::from_utf8_lossy(&line);
            if sender
                .send(text.trim_end_matches(['\n', '\r']).to_owned())
                .is_err()
            {
                break;
            }
        }
    })
}

fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    args.iter()
        .copied()
        .find(|arg| !arg.starts_with('-') && !arg.contains('='))
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
