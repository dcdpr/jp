//! Generic process runner abstraction for dependency injection in tests.

use std::{
    io::{BufRead as _, BufReader},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use duct::{ReaderHandle, cmd};

/// How long an interrupted process gets to clean up before it is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(5);

/// How often to check whether it has finished unwinding.
const INTERRUPT_POLL: Duration = Duration::from_millis(50);

/// Stop `handle` the way Ctrl-C would, and kill it if that is not enough.
///
/// `SIGINT` rather than an outright kill because a process that spawned others
/// is the only one that knows how to stop them.
/// `xcodebuild` interrupted tears down its test session, which is what stops
/// the app a UI test was driving; killed outright it leaves that app running on
/// the screen.
fn interrupt(handle: &ReaderHandle) {
    for pid in handle.pids() {
        let _sent = Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status();
    }

    let deadline = Instant::now() + INTERRUPT_GRACE;
    while Instant::now() < deadline {
        if matches!(handle.try_wait(), Ok(Some(_))) {
            return;
        }

        thread::sleep(INTERRUPT_POLL);
    }

    let _killed = handle.kill();
}

/// The exit code of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct ExitCode {
    /// `None` if the process was terminated by a signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<i32>,
}

impl ExitCode {
    /// Create an exit code representing success (code 0).
    #[cfg(test)]
    pub const fn success() -> Self {
        Self { code: Some(0) }
    }

    /// Create an exit code from an integer.
    #[cfg(test)]
    pub const fn from_code(code: i32) -> Self {
        Self { code: Some(code) }
    }

    /// Returns `true` if the exit code represents success (code 0).
    pub const fn is_success(self) -> bool {
        matches!(self.code, Some(0))
    }
}

impl From<Option<i32>> for ExitCode {
    fn from(code: Option<i32>) -> Self {
        Self { code }
    }
}

impl From<std::process::ExitStatus> for ExitCode {
    fn from(status: std::process::ExitStatus) -> Self {
        Self {
            code: status.code(),
        }
    }
}

impl std::fmt::Display for ExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(code) => write!(f, "{code}"),
            None => write!(f, "terminated by signal"),
        }
    }
}

/// Helper for serde `skip_serializing_if` attribute.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_success_exit_code(code: &ExitCode) -> bool {
    (*code).is_success()
}

/// The output of a process execution.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessOutput {
    #[serde(rename = "output", skip_serializing_if = "String::is_empty")]
    pub stdout: String,

    #[serde(rename = "error", skip_serializing_if = "String::is_empty")]
    pub stderr: String,

    #[serde(skip_serializing_if = "is_success_exit_code")]
    pub status: ExitCode,
}

impl ProcessOutput {
    /// Returns `true` if the process exited successfully (status code 0).
    pub fn success(&self) -> bool {
        self.status.is_success()
    }
}

/// Options for running a process.
#[derive(Debug, Default)]
pub struct RunnerOpts<'a> {
    pub env: &'a [(&'a str, &'a str)],
    pub stdin: Option<&'a str>,

    /// macOS Seatbelt profile string for `sandbox-exec -p`.
    /// If set, the command is wrapped in `sandbox-exec`.
    /// Errors if `sandbox-exec` is not available.
    pub macos_sandbox_profile: Option<&'a str>,

    /// If `true`, the process inherits NO environment variables from the
    /// parent.
    /// Only the variables in `env` are set.
    /// Use this for sandboxed processes to prevent leaking secrets via env
    /// vars.
    pub clean_env: bool,
}

/// Trait for running external processes, allowing for dependency injection in
/// tests.
pub trait ProcessRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
    ) -> Result<ProcessOutput, std::io::Error> {
        self.run_with_opts(program, args, working_dir, &RunnerOpts::default())
    }

    fn run_with_env(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        env: &[(&str, &str)],
    ) -> Result<ProcessOutput, std::io::Error> {
        self.run_with_opts(program, args, working_dir, &RunnerOpts {
            env,
            ..Default::default()
        })
    }

    fn run_with_env_and_stdin(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> Result<ProcessOutput, std::io::Error> {
        self.run_with_opts(program, args, working_dir, &RunnerOpts {
            env,
            stdin,
            ..Default::default()
        })
    }

    fn run_with_opts(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        opts: &RunnerOpts<'_>,
    ) -> Result<ProcessOutput, std::io::Error>;

    /// Run `program`, killing it as soon as a line of its output satisfies
    /// `stop`.
    ///
    /// Both streams are merged, because a caller watching for something has no
    /// way to interleave two captures after the fact.
    ///
    /// The default runs to completion and reports that it stopped nothing, so a
    /// runner that cannot stream still answers correctly — only later than a
    /// caller would like.
    fn run_until(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        _stop: &dyn Fn(&str) -> bool,
    ) -> Result<(ProcessOutput, Stopped), std::io::Error> {
        let output = self.run(program, args, working_dir)?;

        Ok((output, Stopped::No))
    }
}

/// Whether a run was cut short or reached its own end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The process was killed because a line matched.
    Yes,
    /// The process ended on its own.
    No,
}

impl Stopped {
    pub const fn is_yes(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// Production implementation that uses duct to run actual external processes.
pub struct DuctProcessRunner;

impl DuctProcessRunner {
    /// Build the actual program and args, wrapping in a sandbox if requested.
    fn resolve_command<'a>(
        program: &'a str,
        args: &'a [&'a str],
        opts: &'a RunnerOpts<'_>,
    ) -> (String, Vec<String>) {
        // macOS sandbox — only available on macOS, silently skipped elsewhere.
        if cfg!(target_os = "macos")
            && let Some(profile) = opts.macos_sandbox_profile
        {
            let mut sandbox_args = vec![
                "-p".to_owned(),
                profile.to_owned(),
                "--".to_owned(),
                program.to_owned(),
            ];
            sandbox_args.extend(args.iter().map(|s| (*s).to_owned()));
            return ("sandbox-exec".to_owned(), sandbox_args);
        }

        (
            program.to_owned(),
            args.iter().map(|s| (*s).to_owned()).collect(),
        )
    }
}

impl ProcessRunner for DuctProcessRunner {
    fn run_with_opts(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        opts: &RunnerOpts<'_>,
    ) -> Result<ProcessOutput, std::io::Error> {
        let (program, args) = Self::resolve_command(program, args, opts);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

        let mut command = cmd(&program, &arg_refs)
            .dir(working_dir)
            .unchecked()
            .stdout_capture()
            .stderr_capture();

        if opts.clean_env {
            // Replace the entire environment — only `opts.env` entries are set.
            let env_map: std::collections::HashMap<_, _> = opts.env.iter().copied().collect();
            command = command.full_env(env_map);
        } else {
            for (key, value) in opts.env {
                command = command.env(key, value);
            }
        }

        if let Some(input) = opts.stdin {
            command = command.stdin_bytes(input.as_bytes());
        }

        let output = command.run()?;

        // Lossy conversion is deliberate: git diff output for text files can
        // contain stray non-UTF-8 bytes (e.g. a latin-1 source file). A strict
        // `from_utf8().unwrap_or_default()` would discard the entire capture on
        // the first invalid byte and silently report no output.
        Ok(ProcessOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: ExitCode::from(output.status),
        })
    }

    fn run_until(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        stop: &dyn Fn(&str) -> bool,
    ) -> Result<(ProcessOutput, Stopped), std::io::Error> {
        let handle = cmd(program, args)
            .dir(working_dir)
            .unchecked()
            .stderr_to_stdout()
            .reader()?;

        let mut reader = BufReader::new(&handle);
        let mut collected = String::new();
        let mut line = String::new();
        let mut stopped = Stopped::No;

        loop {
            line.clear();
            // Lossy for the same reason the captured path is: one stray byte
            // must not discard the rest of the output.
            let mut bytes = Vec::new();
            if reader.read_until(b'\n', &mut bytes)? == 0 {
                break;
            }
            line.push_str(&String::from_utf8_lossy(&bytes));
            collected.push_str(&line);

            if stop(&line) {
                stopped = Stopped::Yes;
                interrupt(&handle);
                break;
            }
        }

        // A killed process has no status of its own worth reporting, and the
        // caller already knows it was killed.
        let status = match handle.try_wait() {
            Ok(Some(output)) => ExitCode::from(output.status),
            _ => ExitCode::from(None),
        };

        Ok((
            ProcessOutput {
                stdout: collected,
                stderr: String::new(),
                status,
            },
            stopped,
        ))
    }
}

#[cfg(test)]
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[cfg(test)]
struct Expectation {
    program: String,
    args: Option<Vec<String>>,

    /// What running the command does: what it printed, or the kind of error
    /// spawning it produced.
    ///
    /// A binary that is not installed fails to spawn, which is a different
    /// outcome from one that ran and exited non-zero and reaches different code
    /// in the caller.
    /// `ErrorKind` rather than `io::Error` because an expectation is stored and
    /// `io::Error` is not `Clone`.
    result: Result<ProcessOutput, std::io::ErrorKind>,
}

#[cfg(test)]
pub struct MockProcessRunner {
    expectations: Arc<Mutex<VecDeque<Expectation>>>,
}

#[cfg(test)]
impl Drop for MockProcessRunner {
    fn drop(&mut self) {
        // Only check if we're not already panicking and this is the last reference
        if !std::thread::panicking() && Arc::strong_count(&self.expectations) == 1 {
            let remaining = self.expectations.lock().unwrap();
            assert!(
                remaining.is_empty(),
                "MockProcessRunner dropped with {} unfulfilled expectation(s). Expected commands: \
                 {:?}",
                remaining.len(),
                remaining
                    .iter()
                    .map(|e| format!("{} {:?}", e.program, e.args))
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[cfg(test)]
impl MockProcessRunner {
    /// Create a simple mock that returns the same output for any command.
    pub fn success(stdout: impl Into<String>) -> Self {
        Self::builder().expect_any().returns(ProcessOutput {
            stdout: stdout.into(),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    }

    /// Create a simple mock that returns an error for any command.
    pub fn error(stderr: impl Into<String>) -> Self {
        Self::builder().expect_any().returns_error(stderr)
    }

    /// Create a mock that expects no commands.
    /// Panics if any command is run.
    pub fn never_called() -> Self {
        Self {
            expectations: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Create a new builder for setting up expectations.
    pub fn builder() -> MockProcessRunnerBuilder {
        MockProcessRunnerBuilder {
            expectations: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Chain another expectation from an existing runner.
    pub fn expect(self, program: impl Into<String>) -> ExpectationBuilder {
        ExpectationBuilder {
            expectations: self.expectations.clone(),
            program: program.into(),
            args: None,
        }
    }
}

#[cfg(test)]
pub struct MockProcessRunnerBuilder {
    expectations: Arc<Mutex<VecDeque<Expectation>>>,
}

#[cfg(test)]
impl MockProcessRunnerBuilder {
    /// Expect a specific command to be run.
    pub fn expect(self, program: impl Into<String>) -> ExpectationBuilder {
        ExpectationBuilder {
            expectations: self.expectations.clone(),
            program: program.into(),
            args: None,
        }
    }

    /// Expect any command (no validation).
    pub fn expect_any(self) -> ExpectationBuilder {
        ExpectationBuilder {
            expectations: self.expectations.clone(),
            program: String::new(),
            args: None,
        }
    }
}

#[cfg(test)]
pub struct ExpectationBuilder {
    expectations: Arc<Mutex<VecDeque<Expectation>>>,
    program: String,
    args: Option<Vec<String>>,
}

#[cfg(test)]
impl ExpectationBuilder {
    /// Expect specific arguments.
    pub fn args(mut self, args: &[&str]) -> Self {
        self.args = Some(args.iter().map(std::string::ToString::to_string).collect());
        self
    }

    /// Set the output to return.
    pub fn returns(self, output: ProcessOutput) -> MockProcessRunner {
        self.returns_result(Ok(output))
    }

    /// Fail to spawn the command, as a binary that is not installed does.
    ///
    /// Distinct from [`returns_error`], which models a command that ran and
    /// exited non-zero.
    /// A caller that handles the two differently cannot be tested with the
    /// other one.
    ///
    /// [`returns_error`]: Self::returns_error
    pub fn fails_to_spawn(self) -> MockProcessRunner {
        self.returns_result(Err(std::io::ErrorKind::NotFound))
    }

    fn returns_result(
        self,
        result: Result<ProcessOutput, std::io::ErrorKind>,
    ) -> MockProcessRunner {
        self.expectations.lock().unwrap().push_back(Expectation {
            program: self.program,
            args: self.args,
            result,
        });

        MockProcessRunner {
            expectations: self.expectations,
        }
    }

    /// Convenience method to return success with stdout.
    pub fn returns_success(self, stdout: impl Into<String>) -> MockProcessRunner {
        self.returns(ProcessOutput {
            stdout: stdout.into(),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    }

    /// Convenience method to return an error with stderr.
    pub fn returns_error(self, stderr: impl Into<String>) -> MockProcessRunner {
        self.returns(ProcessOutput {
            stdout: String::new(),
            stderr: stderr.into(),
            status: ExitCode::from_code(1),
        })
    }
}

#[cfg(test)]
impl ProcessRunner for MockProcessRunner {
    fn run_with_opts(
        &self,
        program: &str,
        args: &[&str],
        _working_dir: &Utf8Path,
        _opts: &RunnerOpts<'_>,
    ) -> Result<ProcessOutput, std::io::Error> {
        let mut expectations = self.expectations.lock().unwrap();

        let expectation = expectations.pop_front().ok_or_else(|| {
            std::io::Error::other(format!(
                "Unexpected command: {program} {args:?} (no more expectations)"
            ))
        })?;

        // Validate program if specified
        if !expectation.program.is_empty() && expectation.program != program {
            return Err(std::io::Error::other(format!(
                "Expected program '{}' but got '{}'",
                expectation.program, program
            )));
        }

        // Validate args if specified
        if let Some(expected_args) = &expectation.args {
            let actual_args: Vec<String> =
                args.iter().map(std::string::ToString::to_string).collect();
            if expected_args != &actual_args {
                return Err(std::io::Error::other(format!(
                    "Expected args {expected_args:?} but got {actual_args:?}"
                )));
            }
        }

        expectation.result.map_err(std::io::Error::from)
    }
}

#[cfg(test)]
impl ProcessRunner for &MockProcessRunner {
    fn run_with_opts(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        opts: &RunnerOpts<'_>,
    ) -> Result<ProcessOutput, std::io::Error> {
        (*self).run_with_opts(program, args, working_dir, opts)
    }
}
