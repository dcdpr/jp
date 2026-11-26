//! Process runners for tests, so no test spawns a real program.

use std::sync::Arc;

use jp_process::{ExitCode, MockProcessRunner, ProcessOutput, ProcessRunner};

/// A runner for a service whose tools run no local command.
///
/// Running one returns an error naming it.
pub(crate) fn no_commands() -> Arc<dyn ProcessRunner> {
    Arc::new(MockProcessRunner::never_called())
}

/// What a program that printed `stdout` and exited successfully produced.
pub(crate) fn printed(stdout: impl Into<String>) -> ProcessOutput {
    ProcessOutput {
        stdout: stdout.into(),
        stderr: String::new(),
        status: ExitCode::success(),
    }
}

/// A runner whose every command prints its arguments as `echo` would: joined by
/// spaces, and ended by a newline.
///
/// Every argument has been through the template, so the output is what the
/// template rendered.
pub(crate) fn echoing() -> Arc<MockProcessRunner> {
    Arc::new(MockProcessRunner::responding(|spec| {
        Ok(printed(spec.args.join(" ") + "\n"))
    }))
}
