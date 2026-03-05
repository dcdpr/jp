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
    };

    (dir, ctx)
}

#[test]
fn test_cargo_format_no_files_to_format() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::success("");
    let result = cargo_format_impl(&ctx, None, &runner).unwrap();

    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn test_cargo_format_with_files() {
    let (_dir, ctx) = ctx();
    let stdout = format!("{root}/src/main.rs\n{root}/src/lib.rs", root = ctx.root);
    let runner = MockProcessRunner::success(stdout);
    let result = cargo_format_impl(&ctx, None, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "Formatted files:\n- src/main.rs\n- src/lib.rs"
    );
}

#[test]
fn test_cargo_format_with_package() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "fmt",
            "--package=my_package",
            "--",
            "--color=never",
            "--files-with-diff",
        ])
        .returns_success("");

    let result = cargo_format_impl(&ctx, Some("my_package".to_string()), &runner).unwrap();

    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn test_cargo_format_without_package_uses_all() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["fmt", "--all", "--", "--color=never", "--files-with-diff"])
        .returns_success("");

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();

    assert_eq!(result.unwrap_content(), "No files to format.");
}

#[test]
fn test_cargo_format_trims_root_path() {
    let (_dir, ctx) = ctx();
    let stdout = format!("{}/crates/tools/src/main.rs", ctx.root);
    let runner = MockProcessRunner::success(stdout);
    let result = cargo_format_impl(&ctx, None, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "Formatted files:\n- crates/tools/src/main.rs"
    );
}

#[test]
fn test_cargo_format_command_failure() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: could not format files".to_string(),
            status: ExitCode::from_code(1),
        });

    let result = cargo_format_impl(&ctx, None, &runner).unwrap();

    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(
                message,
                "Cargo command failed: error: could not format files"
            );
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn test_cargo_format_with_trailing_newline() {
    let (_dir, ctx) = ctx();
    let stdout = format!("{root}/src/main.rs\n{root}/src/lib.rs\n", root = ctx.root);
    let runner = MockProcessRunner::success(stdout);
    let result = cargo_format_impl(&ctx, None, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "Formatted files:\n- src/main.rs\n- src/lib.rs"
    );
}

#[test]
fn test_cargo_format_single_file() {
    let (_dir, ctx) = ctx();
    let stdout = format!("{}/src/main.rs", ctx.root);
    let runner = MockProcessRunner::success(stdout);
    let result = cargo_format_impl(&ctx, None, &runner).unwrap();

    assert_eq!(result.unwrap_content(), "Formatted files:\n- src/main.rs");
}
