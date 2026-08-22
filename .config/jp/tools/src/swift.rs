//! Tools for the macOS app in `apps/macos`.
//!
//! These mirror the `cargo_*` tools: each shells out to a toolchain binary from
//! the repository root and reports diagnostics rather than raw build logs.
//!
//! Every tool that builds brings its inputs up to date first, through
//! [`prepare`], so a fresh checkout needs no setup step.

use jp_tool::Context;

use crate::{
    Tool,
    util::{
        ToolResult,
        runner::{ProcessOutput, ProcessRunner},
        truncate, unknown_tool,
    },
};

mod check;
mod format;
mod report;
mod test;
mod test_ui;

use check::swift_check;
use format::swift_format;
use test::swift_test;
use test_ui::swift_test_ui;

/// Cap for compiler diagnostics embedded in a tool result.
///
/// `xcodebuild` repeats the failing command line in full for every error, so
/// the tail of a broken build is almost entirely noise.
const MAX_DIAGNOSTIC_BYTES: usize = 32_000;

/// The generated Xcode project, relative to the repository root.
const PROJECT_PATH: &str = "apps/macos/JP.xcodeproj";

/// The `XcodeGen` manifest the project is generated from.
const PROJECT_SPEC: &str = "apps/macos/project.yml";

/// The directory the generated project is written into.
const PROJECT_DIR: &str = "apps/macos";

/// The scheme covering the app and its test bundle.
const SCHEME: &str = "JP";

/// Swift sources formatted and linted by these tools.
const SOURCE_PATHS: &[&str] = &[
    "apps/macos/Sources",
    "apps/macos/Tests",
    "apps/macos/UITests",
    "apps/macos/Tools/jpdrive/Sources",
    "apps/macos/Tools/jpdrive/Tests",
];

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("swift_") {
        "check" => swift_check(&ctx, t.opt("configuration")?).await,
        "test" => swift_test(&ctx, t.opt("testname")?, t.opt("target")?).await,
        "test_ui" => swift_test_ui(&ctx, t.opt("tests")?).await,
        // `check` is a call parameter, not a tool config option, so it is read
        // with `opt` rather than `option_or`.
        "format" => swift_format(&ctx, t.opt("check")?.unwrap_or(false)).await,
        _ => unknown_tool(t),
    }
}

/// Build the generated inputs an Xcode build depends on.
///
/// Returns a failure message, or `None` when both steps succeeded.
///
/// Two things are generated rather than committed: the static library with its
/// C header, and the Xcode project.
/// Both steps are idempotent and cheap when already up to date.
///
/// The header cannot be left to the project's own build phase.
/// Xcode scans the bridging header while planning the build, before any script
/// phase runs, so a missing header fails the scan rather than triggering the
/// phase that would have produced it.
fn prepare<R: ProcessRunner>(
    ctx: &Context,
    profile: &str,
    runner: &R,
) -> Result<Option<String>, std::io::Error> {
    // Going through `just` keeps the profile-to-directory mapping and the
    // cbindgen invocation defined in one place.
    let ffi = runner.run("just", &["build-ffi", profile], &ctx.root)?;
    if !ffi.status.is_success() {
        return Ok(Some(format!(
            "Building `jp_ffi` failed:\n\n```\n{}\n```",
            report(&ffi, "just build-ffi")
        )));
    }

    let project = runner.run(
        "xcodegen",
        &["generate", "--spec", PROJECT_SPEC, "--project", PROJECT_DIR],
        &ctx.root,
    )?;
    if !project.status.is_success() {
        return Ok(Some(format!(
            "Generating `{PROJECT_PATH}` failed:\n\n```\n{}\n```\n\nIf xcodegen is not installed, \
             install it with `brew install xcodegen`.",
            report(&project, "xcodegen")
        )));
    }

    Ok(None)
}

/// The diagnostics from a process, preferring stdout and falling back to
/// stderr.
///
/// `xcodebuild` reports compiler diagnostics on stdout and its own failures on
/// stderr; most other tools use stderr for both.
/// `label` names the program for the case where it printed nothing at all.
fn report(output: &ProcessOutput, label: &str) -> String {
    let stdout = strip(&output.stdout);
    if !stdout.is_empty() {
        return stdout;
    }

    let stderr = strip(&output.stderr);
    if !stderr.is_empty() {
        return stderr;
    }

    format!(
        "{label} exited with status {} and no diagnostics.",
        output.status
    )
}

/// Strip ANSI escapes and trim, capping the result.
fn strip(output: &str) -> String {
    let stripped = strip_ansi_escapes::strip_str(output);
    truncate(stripped.trim(), MAX_DIAGNOSTIC_BYTES)
}

/// The message from a failed outcome.
///
/// Failures come back as `Ok(Outcome::Error { .. })` rather than `Err`, so
/// tests have to reach into the outcome to assert on the message.
///
/// # Panics
///
/// Panics if the outcome is not an error.
#[cfg(test)]
fn error_message(outcome: jp_tool::Outcome) -> String {
    match outcome {
        jp_tool::Outcome::Error { message, .. } => message,
        other => panic!("expected an error outcome, got {other:?}"),
    }
}

#[cfg(test)]
#[path = "swift_tests.rs"]
mod tests;
