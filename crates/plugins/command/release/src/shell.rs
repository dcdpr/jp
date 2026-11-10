//! The programs the release commands run: git, and the project's `bump` and
//! `build`.
//!
//! Each runs in a directory it is given, reads nothing from stdin, and has its
//! output captured, since the plugin's own stdin and stdout carry the host's
//! protocol; a program that runs long, such as `build`, has its output handed
//! on line by line as it comes instead.
//! [`Shell`] holds the environment they run with on top of the plugin's, which
//! a test pins so that the machine's git configuration can't reach in.

use std::{
    ffi::OsStr,
    io::{BufRead as _, BufReader, Read},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
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

    /// Run `program` with `args` in `dir`, handing each line it prints, on
    /// stdout or stderr, to `each` as it comes, and return how it exited.
    ///
    /// Lines lose their line ending, and bytes that aren't UTF-8 are replaced.
    /// After `each` fails, the program still runs to its end, its output read
    /// and dropped so it can't stall on a full pipe, and the error is `each`'s;
    /// otherwise the error says the program couldn't be run at all.
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
        // The lines end once both readers drop their senders.
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

/// Read `pipe` on a thread of its own, sending each line to `sender` without
/// its line ending.
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
