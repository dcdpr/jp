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
fn no_changes_anywhere_reports_nothing_to_format() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();
    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn rustfmt_changes_only_lists_those_files() {
    let (_dir, ctx) = ctx();
    let rustfmt_stdout = format!("{root}/src/main.rs\n{root}/src/lib.rs", root = ctx.root);
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success(rustfmt_stdout)
        .expect("comfort")
        .returns_success("");

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();
    assert_eq!(
        result.unwrap_content(),
        "Formatted files:\n- src/lib.rs\n- src/main.rs"
    );
}

#[test]
fn comfort_changes_only_lists_those_files() {
    let (_dir, ctx) = ctx();
    let comfort_stdout = format!("{}/crates/foo/src/lib.rs", ctx.root);
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success(comfort_stdout);

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();
    assert_eq!(
        result.unwrap_content(),
        "Formatted files:\n- crates/foo/src/lib.rs"
    );
}

#[test]
fn overlapping_changes_are_deduplicated_and_sorted() {
    let (_dir, ctx) = ctx();
    let rustfmt_stdout = format!("{root}/src/b.rs\n{root}/src/a.rs", root = ctx.root);
    let comfort_stdout = format!("{root}/src/a.rs\n{root}/src/c.rs", root = ctx.root);
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success(rustfmt_stdout)
        .expect("comfort")
        .returns_success(comfort_stdout);

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();
    assert_eq!(
        result.unwrap_content(),
        "Formatted files:\n- src/a.rs\n- src/b.rs\n- src/c.rs"
    );
}

#[test]
fn with_package_argument_is_passed_through_to_both_tools() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "fmt",
            "--package=my_pkg",
            "--",
            "--color=never",
            "--files-with-diff",
        ])
        .returns_success("")
        .expect("comfort")
        .args(&[
            "--list-changed",
            "--format-markdown",
            "--reference-links",
            "--language",
            "rust",
            "--package",
            "my_pkg",
        ])
        .returns_success("");

    let result = cargo_format_impl(&ctx, Some("my_pkg"), &runner).unwrap();
    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn without_package_uses_workspace_scope_on_both_tools() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["fmt", "--all", "--", "--color=never", "--files-with-diff"])
        .returns_success("")
        .expect("comfort")
        .args(&[
            "--list-changed",
            "--format-markdown",
            "--reference-links",
            "--language",
            "rust",
            "--workspace",
        ])
        .returns_success("");

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();
    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn rustfmt_failure_short_circuits_before_running_comfort() {
    let (_dir, ctx) = ctx();
    // Single expectation: comfort should never be reached.
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: could not format files".to_owned(),
            status: ExitCode::from_code(1),
        });

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "cargo fmt failed: error: could not format files");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn comfort_failure_is_reported_even_when_rustfmt_succeeded() {
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

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "comfort failed: comfort: parse error");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn trailing_newlines_in_output_are_tolerated() {
    let (_dir, ctx) = ctx();
    let rustfmt_stdout = format!("{root}/src/main.rs\n", root = ctx.root);
    let comfort_stdout = format!("{root}/src/lib.rs\n\n", root = ctx.root);
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success(rustfmt_stdout)
        .expect("comfort")
        .returns_success(comfort_stdout);

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();
    assert_eq!(
        result.unwrap_content(),
        "Formatted files:\n- src/lib.rs\n- src/main.rs"
    );
}
