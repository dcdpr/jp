//! Talking to `jpdrive`, the accessibility driver the app tools read and act
//! through.
//!
//! The driver speaks JSON on stdout for both results and errors, distinguished
//! by exit status.
//! This module owns finding it, and turning a failed run into something a
//! caller can act on; interpreting a successful one belongs to whichever tool
//! asked.

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;

use crate::{
    Error,
    util::runner::{ProcessOutput, ProcessRunner},
};

/// The `jpdrive` package, relative to the repository root.
const PACKAGE: &str = "apps/macos/Tools/jpdrive";

/// Build the driver and return its binary.
pub(crate) fn locate(root: &Utf8Path, runner: &dyn ProcessRunner) -> Result<Utf8PathBuf, Error> {
    let build = runner
        .run("just", &["build-drive"], root)
        .map_err(|e| format!("Failed to spawn `just build-drive`: {e}"))?;
    if !build.success() {
        return Err(format!("`just build-drive` failed:\n\n```\n{}\n```", said(&build)).into());
    }

    // The binary sits under an architecture-specific directory, so SwiftPM is
    // asked where rather than guessed at.
    let path = runner
        .run(
            "swift",
            &[
                "build",
                "--package-path",
                PACKAGE,
                "-c",
                "release",
                "--show-bin-path",
            ],
            root,
        )
        .map_err(|e| format!("Failed to spawn `swift build --show-bin-path`: {e}"))?;
    if !path.success() {
        return Err(format!(
            "`swift build --show-bin-path` failed:\n\n```\n{}\n```",
            said(&path)
        )
        .into());
    }

    let bin = Utf8PathBuf::from(path.stdout.trim()).join("jpdrive");
    if !bin.is_file() {
        return Err(format!("`just build-drive` left no driver at {bin}.").into());
    }

    Ok(bin)
}

/// Everything a failed command said, on whichever stream it said it.
///
/// `just` reports only that a recipe exited non-zero; the compiler diagnostics
/// that explain why are on the recipe's own stdout.
/// Quoting stderr alone leaves a build failure reading `recipe failed with exit
/// code 1` and nothing else.
fn said(output: &ProcessOutput) -> String {
    let mut said = String::new();
    for stream in [output.stdout.trim_end(), output.stderr.trim_end()] {
        if stream.is_empty() {
            continue;
        }

        if !said.is_empty() {
            said.push('\n');
        }
        said.push_str(stream);
    }

    if said.is_empty() {
        return "(it said nothing on either stream)".to_owned();
    }

    said
}

/// The driver's error document.
#[derive(Debug, Deserialize)]
struct ErrorDocument {
    error: DriverError,
}

#[derive(Debug, Deserialize)]
struct DriverError {
    kind: String,
    message: String,
    hint: Option<String>,
}

/// Turn a failed driver run into something actionable.
///
/// `command` names the subcommand that failed, so a report says which one.
///
/// Falls back to the raw streams when the document does not parse, because a
/// driver that failed before it could write JSON is exactly when the raw output
/// is worth reading.
pub(crate) fn failure(command: &str, stdout: &str, stderr: &str) -> String {
    let Ok(document) = serde_json::from_str::<ErrorDocument>(stdout) else {
        return format!(
            "`jpdrive {command}` failed and reported nothing \
             parseable.\n\nstdout:\n\n```\n{}\n```\n\nstderr:\n\n```\n{}\n```",
            stdout.trim_end(),
            stderr.trim_end()
        );
    };

    let mut message = format!(
        "`jpdrive {command}` failed ({}): {}",
        document.error.kind, document.error.message
    );
    if let Some(hint) = document.error.hint {
        message.push_str(&format!("\n\nHint: {hint}"));
    }

    message
}

/// The kind the driver named, when it answered a parseable error document.
pub(crate) fn kind(stdout: &str) -> Option<String> {
    serde_json::from_str::<ErrorDocument>(stdout)
        .ok()
        .map(|document| document.error.kind)
}

/// Whether the driver refused for lack of an Accessibility grant.
pub(crate) fn is_not_permitted(stdout: &str) -> bool {
    kind(stdout).as_deref() == Some("not_permitted")
}

/// Ask the driver what it can see, after it has refused to act.
///
/// Returns `None` when the diagnostic itself fails: a broken diagnostic must
/// not replace the error it was meant to explain.
///
/// Quoted verbatim rather than summarised.
/// Whether a grant given to a terminal reaches a tool that terminal started is
/// undocumented by Apple and has to be measured, so the ancestor chain and the
/// probe are evidence for a reader, not a verdict to restate.
pub(crate) fn diagnose_permission(
    driver: &Utf8Path,
    pid: u32,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Option<String> {
    let report = runner
        .run(
            driver.as_str(),
            &["doctor", "--pid", &pid.to_string()],
            root,
        )
        .ok()?;

    Some(format!(
        "\n\n`jpdrive doctor` reports:\n\n```json\n{}\n```",
        report.stdout.trim_end()
    ))
}

/// Run `command`, and turn a refusal into a message carrying the diagnosis.
pub(crate) fn describe_failure(
    command: &str,
    driver: &Utf8Path,
    pid: u32,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
    stdout: &str,
    stderr: &str,
) -> String {
    let mut message = failure(command, stdout, stderr);
    if is_not_permitted(stdout)
        && let Some(diagnosis) = diagnose_permission(driver, pid, root, runner)
    {
        message.push_str(&diagnosis);
    }

    message
}

#[cfg(all(test, unix))]
#[path = "driver_tests.rs"]
mod tests;
