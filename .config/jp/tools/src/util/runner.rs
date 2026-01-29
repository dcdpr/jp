//! Generic process runner abstraction for dependency injection in tests.

use camino::Utf8Path;
use duct::cmd;

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

/// Trait for running external processes, allowing for dependency injection in
/// tests.
pub trait ProcessRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
    ) -> Result<ProcessOutput, std::io::Error> {
        self.run_with_env(program, args, working_dir, &[])
    }

    fn run_with_env(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        env: &[(&str, &str)],
    ) -> Result<ProcessOutput, std::io::Error> {
        self.run_with_env_and_stdin(program, args, working_dir, env, None)
    }

    fn run_with_env_and_stdin(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> Result<ProcessOutput, std::io::Error>;
}

/// Production implementation that uses duct to run actual external processes.
pub struct DuctProcessRunner;

impl ProcessRunner for DuctProcessRunner {
    fn run_with_env_and_stdin(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> Result<ProcessOutput, std::io::Error> {
        let mut command = cmd(program, args)
            .dir(working_dir)
            .unchecked()
            .stdout_capture()
            .stderr_capture();

        for (key, value) in env {
            command = command.env(key, value);
        }

        if let Some(input) = stdin {
            command = command.stdin_bytes(input.as_bytes());
        }

        let output = command.run()?;

        Ok(ProcessOutput {
            stdout: String::from_utf8(output.stdout).unwrap_or_default(),
            stderr: String::from_utf8(output.stderr).unwrap_or_default(),
            status: ExitCode::from(output.status),
        })
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
    output: ProcessOutput,
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
        self.expectations.lock().unwrap().push_back(Expectation {
            program: self.program,
            args: self.args,
            output,
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
    fn run_with_env_and_stdin(
        &self,
        program: &str,
        args: &[&str],
        _working_dir: &Utf8Path,
        _env: &[(&str, &str)],
        _stdin: Option<&str>,
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

        Ok(expectation.output)
    }
}

#[cfg(test)]
impl ProcessRunner for &MockProcessRunner {
    fn run_with_env_and_stdin(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> Result<ProcessOutput, std::io::Error> {
        (*self).run_with_env_and_stdin(program, args, working_dir, env, stdin)
    }
}
