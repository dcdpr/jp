use camino_tempfile::tempdir;
use jp_tool::Action;
use pretty_assertions::assert_eq;

use super::*;
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput};

#[test]
fn test_cargo_check_with_warnings() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

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
        .expect_any()
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr,
            status: ExitCode::success(),
        });

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
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let runner = MockProcessRunner::success("");
    let result = cargo_check_impl(&ctx, None, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}
