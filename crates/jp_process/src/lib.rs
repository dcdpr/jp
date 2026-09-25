//! Running a program to completion and collecting what it printed.
//!
//! [`ProcessRunner`] is the seam every run-once command in JP goes through: a
//! local tool, an argument formatter, a `git` call from a maintenance tool.
//! [`SystemProcessRunner`] spawns the real process; with the `mock` feature,
//! `MockProcessRunner` answers from a script instead, so the code deciding
//! *what* to run is tested without spawning anything.
//!
//! The runner is synchronous.
//! Async callers run it on a blocking thread, and stop it through
//! [`Watch::cancellation`] rather than by dropping a future.
//!
//! Long-lived processes are out of scope: an MCP server, a plugin, or an editor
//! talks to JP while it runs, which this does not model.

use std::{fmt, io, process::ExitStatus, sync::Arc, time::Duration};

use camino::{Utf8Path, Utf8PathBuf};
use tokio_util::sync::CancellationToken;

#[cfg(any(test, feature = "mock"))]
mod mock;
mod system;

#[cfg(any(test, feature = "mock"))]
pub use mock::{ExpectationBuilder, MockProcessRunner, MockProcessRunnerBuilder};
pub use system::SystemProcessRunner;

/// A program to run: what, with which arguments, where, and with what input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    /// The program, found on `PATH` unless it is a path.
    pub program: String,

    /// Arguments, passed as-is: nothing here is interpreted by a shell.
    pub args: Vec<String>,

    /// The working directory.
    pub dir: Utf8PathBuf,

    /// Variables set on top of the environment.
    pub env: Vec<(String, String)>,

    /// Whether the process starts from an empty environment, so it sees only
    /// [`env`].
    ///
    /// [`env`]: Self::env
    pub clean_env: bool,

    /// Written to the process's standard input, which is then closed.
    ///
    /// `None` leaves standard input inherited.
    pub stdin: Option<String>,

    /// Whether the process leads a process group of its own.
    ///
    /// A Ctrl-C at the terminal is sent to the terminal's foreground group, so
    /// a process in a group of its own does not receive it: whoever started it
    /// decides what stops it.
    /// Otherwise it shares this process's group, and a Ctrl-C reaches both.
    ///
    /// Unix only; elsewhere the process always shares the console.
    pub own_process_group: bool,
}

impl ProcessSpec {
    /// Run `program` with `args` in `dir`, in the inherited environment.
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
        dir: impl Into<Utf8PathBuf>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            dir: dir.into(),
            env: Vec::new(),
            clean_env: false,
            stdin: None,
            own_process_group: false,
        }
    }

    fn with_opts(program: &str, args: &[&str], dir: &Utf8Path, opts: &RunnerOpts<'_>) -> Self {
        Self {
            env: opts
                .env
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            clean_env: opts.clean_env,
            stdin: opts.stdin.map(str::to_owned),
            ..Self::new(program, args.iter().copied(), dir)
        }
    }
}

/// The program followed by its arguments, separated by spaces, for a message
/// naming the command.
impl fmt::Display for ProcessSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.program)?;
        for arg in &self.args {
            write!(f, " {arg}")?;
        }
        Ok(())
    }
}

/// Environment and input for one of the [`ProcessRunner`] shorthands.
#[derive(Debug, Default)]
pub struct RunnerOpts<'a> {
    /// Variables set on top of the environment.
    pub env: &'a [(&'a str, &'a str)],

    /// Written to standard input, which is then closed.
    pub stdin: Option<&'a str>,

    /// Whether the process starts from an empty environment, so it sees only
    /// `env`.
    ///
    /// For sandboxed processes, which must not see secrets the parent holds in
    /// its environment.
    pub clean_env: bool,
}

/// Receives each line a process prints, without its line ending.
pub type LineSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Decides from a line a process printed whether to stop it.
pub type LineMatch = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// What to observe while a process runs, and what stops it early.
#[derive(Clone, Default)]
pub struct Watch {
    /// Called with each line of standard error as it arrives.
    ///
    /// Called from a reader thread, so it must not block: the process cannot
    /// make progress once its pipe is full.
    pub stderr_lines: Option<LineSink>,

    /// Stops the process at the first line of either stream it matches.
    pub stop_when: Option<LineMatch>,

    /// Stops the process when cancelled.
    pub cancellation: Option<CancellationToken>,

    /// How long a process asked to stop gets to exit on its own before it is
    /// killed.
    ///
    /// Zero kills it at once.
    /// Otherwise it is interrupted first, as Ctrl-C would, because a process
    /// that started others is the only one that knows how to stop them.
    /// Windows has no interrupt to send, so there it is always killed at once.
    pub grace: Duration,
}

impl fmt::Debug for Watch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watch")
            .field("stderr_lines", &self.stderr_lines.is_some())
            .field("stop_when", &self.stop_when.is_some())
            .field("cancellation", &self.cancellation)
            .field("grace", &self.grace)
            .finish()
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The process exited on its own, and both its streams were read to the
    /// end.
    Exited,

    /// A line matched [`Watch::stop_when`], and the process was stopped.
    Stopped,

    /// [`Watch::cancellation`] fired, and the process was stopped.
    Cancelled,
}

/// What a run printed, and how it ended.
#[derive(Debug, Clone)]
pub struct Finished {
    /// What the process printed.
    ///
    /// A stopped process's output ends where reading stopped, which can be
    /// short of what it printed.
    pub output: ProcessOutput,

    /// How the run ended.
    pub ended: Ended,
}

/// The exit status of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitCode {
    /// `None` if the process was ended by a signal, or its status is unknown.
    code: Option<i32>,
}

impl ExitCode {
    /// The status of a process that succeeded.
    #[must_use]
    pub const fn success() -> Self {
        Self { code: Some(0) }
    }

    /// The status of a process that exited with `code`.
    #[must_use]
    pub const fn from_code(code: i32) -> Self {
        Self { code: Some(code) }
    }

    /// The code the process exited with, if it exited on its own.
    #[must_use]
    pub const fn code(self) -> Option<i32> {
        self.code
    }

    /// Whether the process exited with code 0.
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self.code, Some(0))
    }
}

impl From<Option<i32>> for ExitCode {
    fn from(code: Option<i32>) -> Self {
        Self { code }
    }
}

impl From<ExitStatus> for ExitCode {
    fn from(status: ExitStatus) -> Self {
        Self {
            code: status.code(),
        }
    }
}

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "{code}"),
            None => write!(f, "terminated by signal"),
        }
    }
}

/// What a process printed, and how it exited.
///
/// Both streams are decoded lossily: one byte of invalid UTF-8 costs that byte,
/// not the rest of the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    /// Standard output.
    pub stdout: String,

    /// Standard error.
    pub stderr: String,

    /// How the process exited.
    pub status: ExitCode,
}

impl ProcessOutput {
    /// Whether the process exited with code 0.
    #[must_use]
    pub fn success(&self) -> bool {
        self.status.is_success()
    }
}

/// Runs a program to completion and collects what it printed.
///
/// [`execute`] is the one method an implementation provides; the rest are
/// shorthands for it.
///
/// [`execute`]: Self::execute
pub trait ProcessRunner: Send + Sync {
    /// Run `spec` until it exits, or until `watch` stops it.
    ///
    /// # Errors
    ///
    /// Returns an error when the process could not be started, such as a
    /// program that is not installed.
    /// A process that ran and failed is an `Ok` whose status says so.
    fn execute(&self, spec: &ProcessSpec, watch: &Watch) -> io::Result<Finished>;

    /// Run `program` with `args` in `dir`, in the inherited environment.
    ///
    /// # Errors
    ///
    /// See [`execute`].
    ///
    /// [`execute`]: Self::execute
    fn run(&self, program: &str, args: &[&str], dir: &Utf8Path) -> io::Result<ProcessOutput> {
        self.run_with_opts(program, args, dir, &RunnerOpts::default())
    }

    /// Run `program` with `env` set on top of the inherited environment.
    ///
    /// # Errors
    ///
    /// See [`execute`].
    ///
    /// [`execute`]: Self::execute
    fn run_with_env(
        &self,
        program: &str,
        args: &[&str],
        dir: &Utf8Path,
        env: &[(&str, &str)],
    ) -> io::Result<ProcessOutput> {
        self.run_with_opts(program, args, dir, &RunnerOpts {
            env,
            ..Default::default()
        })
    }

    /// Run `program` with `env` set, and `stdin` written to its input.
    ///
    /// # Errors
    ///
    /// See [`execute`].
    ///
    /// [`execute`]: Self::execute
    fn run_with_env_and_stdin(
        &self,
        program: &str,
        args: &[&str],
        dir: &Utf8Path,
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> io::Result<ProcessOutput> {
        self.run_with_opts(program, args, dir, &RunnerOpts {
            env,
            stdin,
            ..Default::default()
        })
    }

    /// Run `program` with the environment and input `opts` describe.
    ///
    /// # Errors
    ///
    /// See [`execute`].
    ///
    /// [`execute`]: Self::execute
    fn run_with_opts(
        &self,
        program: &str,
        args: &[&str],
        dir: &Utf8Path,
        opts: &RunnerOpts<'_>,
    ) -> io::Result<ProcessOutput> {
        let spec = ProcessSpec::with_opts(program, args, dir, opts);
        self.execute(&spec, &Watch::default())
            .map(|finished| finished.output)
    }

    /// Run `program`, stopping it at the first line of its output that
    /// satisfies `stop`, and giving it `grace` to exit before it is killed.
    ///
    /// # Errors
    ///
    /// See [`execute`].
    ///
    /// [`execute`]: Self::execute
    fn run_until(
        &self,
        program: &str,
        args: &[&str],
        dir: &Utf8Path,
        stop: LineMatch,
        grace: Duration,
    ) -> io::Result<Finished> {
        let spec = ProcessSpec::new(program, args.iter().copied(), dir);
        self.execute(&spec, &Watch {
            stop_when: Some(stop),
            grace,
            ..Watch::default()
        })
    }
}

impl<T: ProcessRunner + ?Sized> ProcessRunner for &T {
    fn execute(&self, spec: &ProcessSpec, watch: &Watch) -> io::Result<Finished> {
        (**self).execute(spec, watch)
    }
}

impl<T: ProcessRunner + ?Sized> ProcessRunner for Arc<T> {
    fn execute(&self, spec: &ProcessSpec, watch: &Watch) -> io::Result<Finished> {
        (**self).execute(spec, watch)
    }
}

impl<T: ProcessRunner + ?Sized> ProcessRunner for Box<T> {
    fn execute(&self, spec: &ProcessSpec, watch: &Watch) -> io::Result<Finished> {
        (**self).execute(spec, watch)
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
