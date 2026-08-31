use std::{io, sync::Mutex};

use camino::Utf8Path;
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_tool::{Action, Context, Outcome};
use pretty_assertions::assert_eq;

use super::*;
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput, RunnerOpts};

fn ctx() -> (Utf8TempDir, Context) {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    (dir, ctx)
}

#[test]
fn test_cargo_check_with_warnings() {
    let (_dir, ctx) = ctx();

    let stderr = indoc::formatdoc! {r#"
            warning: unused `std::result::Result` that must be used
             --> src/main.rs:2:5
              |
            2 |     std::env::var("FOO");
              |     ^^^^^^^^^^^^^^^^^^^^
              |
              = note: this `Result` may be an `Err` variant, which should be handled
              = note: `#[warn(unused_must_use)]` (part of `#[warn(unused)]`) on by default
            help: use `let _ = ...` to ignore the resulting value
              |
            2 |     let _ = std::env::var("FOO");
              |     +++++++
            "#};

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr,
            status: ExitCode::success(),
        })
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {r#"
            ```
            warning: unused `std::result::Result` that must be used
             --> src/main.rs:2:5
              |
            2 |     std::env::var("FOO");
              |     ^^^^^^^^^^^^^^^^^^^^
              |
              = note: this `Result` may be an `Err` variant, which should be handled
              = note: `#[warn(unused_must_use)]` (part of `#[warn(unused)]`) on by default
            help: use `let _ = ...` to ignore the resulting value
              |
            2 |     let _ = std::env::var("FOO");
              |     +++++++
            ```
        "#});
}

#[test]
fn test_cargo_check_no_warnings() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

#[test]
fn clean_clippy_with_comfort_drift_appends_note() {
    let (_dir, ctx) = ctx();
    let comfort_stdout = format!("{root}/src/lib.rs\n{root}/src/main.rs", root = ctx.root);

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: comfort_stdout,
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner).unwrap();

    // The header is clippy-scoped, not a blanket "Check succeeded", so it does
    // not contradict the drift note below it.
    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            Doc comments in the following files are badly formatted. Run `cargo_fmt` to auto-fix them:
            - src/lib.rs
            - src/main.rs"});
}

#[test]
fn clippy_warnings_and_comfort_drift_are_both_reported() {
    let (_dir, ctx) = ctx();
    let comfort_stdout = format!("{root}/src/lib.rs", root = ctx.root);

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "warning: something".to_owned(),
            status: ExitCode::success(),
        })
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: comfort_stdout,
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            ```
            warning: something
            ```

            Doc comments in the following files are badly formatted. Run `cargo_fmt` to auto-fix them:
            - src/lib.rs"});
}

#[test]
fn comfort_drift_listing_is_bounded() {
    let (_dir, ctx) = ctx();
    let comfort_stdout = (0..4_000)
        .map(|i| format!("{root}/src/generated/file_{i}.rs", root = ctx.root))
        .collect::<Vec<_>>()
        .join("\n");

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: comfort_stdout,
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let content = cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner)
        .unwrap()
        .unwrap_content();

    assert!(
        content.len() < MAX_DIAGNOSTIC_BYTES + 200,
        "drift note grew to {} bytes",
        content.len()
    );
    assert!(
        content.contains("[Truncated: showing"),
        "got tail: {}",
        &content[content.len() - 100..]
    );
}

#[test]
fn comfort_real_failure_is_reported_as_error() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "comfort: parse error".to_owned(),
            status: ExitCode::from_code(2),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "comfort failed: comfort: parse error");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn clippy_failure_short_circuits_before_running_comfort() {
    let (_dir, ctx) = ctx();
    // Single expectation: comfort should never be reached.
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: build failed".to_owned(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "Cargo command failed: error: build failed");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn package_scope_is_passed_through_to_both_tools() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "clippy",
            "--color=never",
            "--package=my_pkg",
            "--quiet",
            "--all-targets",
            "--all-features",
        ])
        .returns_success("")
        .expect("comfort")
        .args(&[
            "--check",
            "--list-changed",
            "--format-markdown",
            "--reference-links",
            "--prune-reference-links",
            "--language",
            "rust",
            "--package",
            "my_pkg",
        ])
        .returns_success("");

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        Some("my_pkg"),
        false,
        &runner,
    )
    .unwrap();
    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// The configured profile has to reach clippy.
///
/// Without it the run lands in whichever profile directory the developer's own
/// builds use, where it takes the build lock and rewrites the fingerprints they
/// are relying on.
#[test]
fn profile_is_passed_through_to_clippy() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "clippy",
            "--color=never",
            "--workspace",
            "--quiet",
            "--all-targets",
            "--all-features",
            "--profile=agent",
        ])
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        Some("agent"),
        None,
        false,
        &runner,
    )
    .unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// Unconfigured means no flag at all, leaving the profile to cargo.
#[test]
fn clippy_gets_no_profile_flag_unless_configured() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "clippy",
            "--color=never",
            "--workspace",
            "--quiet",
            "--all-targets",
            "--all-features",
        ])
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// `CARGO_UNSTABLE_CHECKSUM_FRESHNESS` has to reach cargo.
///
/// Sibling git worktrees share a target directory, and mtime-based freshness
/// lets one checkout serve the other's stale artifacts; content checksums are
/// what prevent that.
/// `MockProcessRunner` validates args but ignores the environment, so nothing
/// else here would notice the variable going missing.
#[test]
fn checksum_freshness_reaches_cargo() {
    let (_dir, ctx) = ctx();

    let runner: CallCapturingRunner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .into();

    cargo_check_impl(&ctx.root, "-W warnings", None, None, true, &runner).unwrap();

    let call = runner
        .call_with_arg("clippy")
        .expect("the clippy pass must have run");

    assert_eq!(
        call.env
            .iter()
            .find(|(key, _)| key == "CARGO_UNSTABLE_CHECKSUM_FRESHNESS")
            .map(|(_, value)| value.as_str()),
        Some("true"),
    );
}

/// Off unless opted into, so the tools also work on stable cargo.
#[test]
fn checksum_freshness_is_absent_unless_opted_into() {
    let (_dir, ctx) = ctx();

    let runner: CallCapturingRunner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .into();

    cargo_check_impl(&ctx.root, "-W warnings", None, None, false, &runner).unwrap();

    let call = runner
        .call_with_arg("clippy")
        .expect("the clippy pass must have run");

    assert!(
        !call
            .env
            .iter()
            .any(|(key, _)| key == "CARGO_UNSTABLE_CHECKSUM_FRESHNESS"),
        "the clippy pass must not require nightly cargo unless asked to",
    );
}

/// One recorded subprocess invocation.
struct CapturedCall {
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// A runner that records every call's args and environment.
///
/// `EnvCapturingRunner` in `cargo/test_tests.rs` keeps only the most recent
/// call's environment; `cargo_check` makes two, so the clippy pass has to be
/// picked out rather than assumed to be last.
struct CallCapturingRunner {
    inner: MockProcessRunner,
    calls: Mutex<Vec<CapturedCall>>,
}

impl From<MockProcessRunner> for CallCapturingRunner {
    fn from(inner: MockProcessRunner) -> Self {
        Self {
            inner,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl CallCapturingRunner {
    /// The first recorded call whose first argument is `arg`.
    fn call_with_arg(&self, arg: &str) -> Option<CapturedCall> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .find(|call| call.args.first().is_some_and(|first| first == arg))
            .map(|call| CapturedCall {
                args: call.args.clone(),
                env: call.env.clone(),
            })
    }
}

impl ProcessRunner for CallCapturingRunner {
    fn run_with_opts(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        opts: &RunnerOpts<'_>,
    ) -> Result<ProcessOutput, io::Error> {
        self.calls.lock().unwrap().push(CapturedCall {
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            env: opts
                .env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        });

        self.inner.run_with_opts(program, args, working_dir, opts)
    }
}
