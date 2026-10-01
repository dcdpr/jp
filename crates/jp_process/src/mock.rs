//! A runner that answers from a script instead of spawning anything.

use std::{
    collections::VecDeque,
    fmt, io,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use tokio_util::sync::CancellationToken;

use crate::{Ended, ExitCode, Finished, ProcessOutput, ProcessRunner, ProcessSpec, Watch};

/// Answers a run from what the command was, rather than from a queue.
type Responder = dyn Fn(&ProcessSpec) -> io::Result<ProcessOutput> + Send + Sync;

/// One command the mock expects, in order, and what it answers with.
struct Expectation {
    /// The program, or empty to accept any.
    program: String,

    /// The arguments, or `None` to accept any.
    args: Option<Vec<String>>,

    /// What running the command does: what it printed, or the kind of error
    /// spawning it produced.
    ///
    /// A binary that is not installed fails to spawn, which is a different
    /// outcome from one that ran and exited non-zero, and reaches different
    /// code in the caller.
    /// `ErrorKind` rather than `io::Error` because an expectation is stored and
    /// `io::Error` is not `Clone`.
    result: Result<ProcessOutput, io::ErrorKind>,
}

/// A [`ProcessRunner`] that answers from a script, and records every run.
///
/// Answers come either from a queue of expected commands, each matched in
/// order, or from a function of the command.
/// Dropping a queued mock with expectations left over fails the test, so a
/// command the code never ran is noticed.
///
/// A run is observed as the real runner would: each line of scripted standard
/// error reaches [`Watch::stderr_lines`], a line matching [`Watch::stop_when`]
/// ends the run as stopped, and a cancelled [`Watch::cancellation`] ends it as
/// cancelled.
pub struct MockProcessRunner {
    expectations: Arc<Mutex<VecDeque<Expectation>>>,
    responder: Option<Arc<Responder>>,
    calls: Arc<Mutex<Vec<ProcessSpec>>>,
}

impl fmt::Debug for MockProcessRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockProcessRunner")
            .field("expectations", &lock(&self.expectations).len())
            .field("responder", &self.responder.is_some())
            .field("calls", &*lock(&self.calls))
            .finish()
    }
}

impl Drop for MockProcessRunner {
    fn drop(&mut self) {
        // Only the last handle checks, and not while a test is already failing.
        if std::thread::panicking() || Arc::strong_count(&self.expectations) != 1 {
            return;
        }

        let remaining = lock(&self.expectations);
        assert!(
            remaining.is_empty(),
            "MockProcessRunner dropped with {} unfulfilled expectation(s). Expected commands: {:?}",
            remaining.len(),
            remaining
                .iter()
                .map(|e| format!("{} {:?}", e.program, e.args))
                .collect::<Vec<_>>()
        );
    }
}

impl MockProcessRunner {
    /// A mock that answers any one command with `stdout`, successfully.
    pub fn success(stdout: impl Into<String>) -> Self {
        Self::builder().expect_any().returns_success(stdout)
    }

    /// A mock that answers any one command with `stderr`, and exit code 1.
    pub fn error(stderr: impl Into<String>) -> Self {
        Self::builder().expect_any().returns_error(stderr)
    }

    /// A mock that expects no command at all.
    ///
    /// Running one returns an error naming it.
    #[must_use]
    pub fn never_called() -> Self {
        Self::builder().build()
    }

    /// A mock that answers every command with what `respond` returns for it.
    ///
    /// For code that runs the same program more than once, answering
    /// differently depending on its arguments.
    pub fn responding(
        respond: impl Fn(&ProcessSpec) -> io::Result<ProcessOutput> + Send + Sync + 'static,
    ) -> Self {
        Self {
            expectations: Arc::new(Mutex::new(VecDeque::new())),
            responder: Some(Arc::new(respond)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start a queue of expected commands.
    #[must_use]
    pub fn builder() -> MockProcessRunnerBuilder {
        MockProcessRunnerBuilder {
            expectations: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Expect `program` to run after the commands already expected.
    pub fn expect(self, program: impl Into<String>) -> ExpectationBuilder {
        ExpectationBuilder {
            expectations: Arc::clone(&self.expectations),
            program: program.into(),
            args: None,
        }
    }

    /// Every command run so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<ProcessSpec> {
        lock(&self.calls).clone()
    }

    /// The answer to `spec`: the responder's, or the next expectation's.
    fn answer(&self, spec: &ProcessSpec) -> io::Result<ProcessOutput> {
        if let Some(respond) = &self.responder {
            return respond(spec);
        }

        let expectation = lock(&self.expectations).pop_front().ok_or_else(|| {
            io::Error::other(format!("Unexpected command: {spec} (no more expectations)"))
        })?;

        if !expectation.program.is_empty() && expectation.program != spec.program {
            return Err(io::Error::other(format!(
                "Expected program '{}' but got '{}'",
                expectation.program, spec.program
            )));
        }

        if let Some(args) = &expectation.args
            && args != &spec.args
        {
            return Err(io::Error::other(format!(
                "Expected args {args:?} but got {:?}",
                spec.args
            )));
        }

        expectation.result.map_err(io::Error::from)
    }
}

impl ProcessRunner for MockProcessRunner {
    fn execute(&self, spec: &ProcessSpec, watch: &Watch) -> io::Result<Finished> {
        lock(&self.calls).push(spec.clone());
        let output = self.answer(spec)?;

        if let Some(sink) = &watch.stderr_lines {
            output.stderr.lines().for_each(|line| sink(line));
        }

        let stopped = watch.stop_when.as_ref().is_some_and(|stop| {
            output
                .stdout
                .lines()
                .chain(output.stderr.lines())
                .any(|line| stop(line))
        });
        let ended = if watch
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            Ended::Cancelled
        } else if stopped {
            Ended::Stopped
        } else {
            Ended::Exited
        };

        Ok(Finished { output, ended })
    }
}

/// Builds the queue of commands a [`MockProcessRunner`] expects.
pub struct MockProcessRunnerBuilder {
    expectations: Arc<Mutex<VecDeque<Expectation>>>,
}

impl MockProcessRunnerBuilder {
    /// Expect `program` to run next.
    pub fn expect(self, program: impl Into<String>) -> ExpectationBuilder {
        ExpectationBuilder {
            expectations: self.expectations,
            program: program.into(),
            args: None,
        }
    }

    /// Expect some command to run next, whatever it is.
    #[must_use]
    pub fn expect_any(self) -> ExpectationBuilder {
        self.expect("")
    }

    fn build(self) -> MockProcessRunner {
        MockProcessRunner {
            expectations: self.expectations,
            responder: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

/// Describes one expected command, and what running it does.
pub struct ExpectationBuilder {
    expectations: Arc<Mutex<VecDeque<Expectation>>>,
    program: String,
    args: Option<Vec<String>>,
}

impl ExpectationBuilder {
    /// Expect exactly these arguments.
    #[must_use]
    pub fn args(mut self, args: &[&str]) -> Self {
        self.args = Some(args.iter().map(ToString::to_string).collect());
        self
    }

    /// Answer with `output`.
    #[must_use]
    pub fn returns(self, output: ProcessOutput) -> MockProcessRunner {
        self.returns_result(Ok(output))
    }

    /// Fail to spawn the command, as a program that is not installed does.
    ///
    /// Distinct from [`returns_error`], which models a command that ran and
    /// exited non-zero: a caller that handles the two differently cannot be
    /// tested with the other one.
    ///
    /// [`returns_error`]: Self::returns_error
    #[must_use]
    pub fn fails_to_spawn(self) -> MockProcessRunner {
        self.returns_result(Err(io::ErrorKind::NotFound))
    }

    /// Answer with `stdout`, successfully.
    pub fn returns_success(self, stdout: impl Into<String>) -> MockProcessRunner {
        self.returns(ProcessOutput {
            stdout: stdout.into(),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    }

    /// Answer with `stderr`, and exit code 1.
    pub fn returns_error(self, stderr: impl Into<String>) -> MockProcessRunner {
        self.returns(ProcessOutput {
            stdout: String::new(),
            stderr: stderr.into(),
            status: ExitCode::from_code(1),
        })
    }

    fn returns_result(self, result: Result<ProcessOutput, io::ErrorKind>) -> MockProcessRunner {
        lock(&self.expectations).push_back(Expectation {
            program: self.program,
            args: self.args,
            result,
        });

        MockProcessRunnerBuilder {
            expectations: self.expectations,
        }
        .build()
    }
}

/// Nothing panics while holding the mock's locks, so a poisoned one is whole.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "mock_tests.rs"]
mod tests;
