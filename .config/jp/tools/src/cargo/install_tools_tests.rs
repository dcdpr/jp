use camino_tempfile::{Utf8TempDir, tempdir};
use jp_tool::{Action, Context};
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
fn installs_through_the_just_recipe() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("just")
        .args(&["install-tools"])
        .returns_success("");

    let result = cargo_install_tools_impl(&ctx.root, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "Rebuilt and installed the `jp-tools` binary. The new code takes effect on the next tool \
         call, not this one."
    );
}

/// The rebuilt binary only serves the next call, so the result says so rather
/// than letting a caller conclude their change is already live.
#[test]
fn success_names_when_the_change_takes_effect() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::success("");

    let result = cargo_install_tools_impl(&ctx.root, &runner).unwrap();

    assert!(
        result.unwrap_content().contains("next tool call"),
        "the result should say the new code is not live in this call"
    );
}

#[test]
fn reports_compiler_errors() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::error("error[E0425]: cannot find value `nope` in this scope");

    let error = cargo_install_tools_impl(&ctx.root, &runner).unwrap();

    assert_eq!(
        error_message(error),
        "Rebuilding the tools binary failed:\n\n```\nerror[E0425]: cannot find value `nope` in \
         this scope\n```"
    );
}

/// `cargo` writes its progress to stderr, but a `just` failure before cargo
/// runs can land on stdout, so neither stream is trusted alone.
#[test]
fn falls_back_to_stdout_for_diagnostics() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("just")
        .returns(ProcessOutput {
            stdout: "error: Justfile does not contain recipe `install-tools`".to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let error = cargo_install_tools_impl(&ctx.root, &runner).unwrap();

    assert!(
        error_message(error).contains("does not contain recipe"),
        "expected the stdout diagnostics to be reported"
    );
}

/// The message from a failed outcome.
///
/// # Panics
///
/// Panics if the outcome is not an error.
fn error_message(outcome: jp_tool::Outcome) -> String {
    match outcome {
        jp_tool::Outcome::Error { message, .. } => message,
        other => panic!("expected an error outcome, got {other:?}"),
    }
}
