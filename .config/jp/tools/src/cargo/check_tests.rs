use camino_tempfile::{Utf8TempDir, tempdir};
use jp_tool::{Action, Outcome};
use pretty_assertions::assert_eq;

use super::*;
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput};

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

    let result = cargo_check_impl(&ctx, None, &runner).unwrap();

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

    let result = cargo_check_impl(&ctx, None, &runner).unwrap();

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

    let result = cargo_check_impl(&ctx, None, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            Check succeeded. No warnings or errors found.

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

    let result = cargo_check_impl(&ctx, None, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            ```
            warning: something
            ```

            Doc comments in the following files are badly formatted. Run `cargo_fmt` to auto-fix them:
            - src/lib.rs"});
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

    let result = cargo_check_impl(&ctx, None, &runner).unwrap();
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

    let result = cargo_check_impl(&ctx, None, &runner).unwrap();
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

    let result = cargo_check_impl(&ctx, Some("my_pkg"), &runner).unwrap();
    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}
