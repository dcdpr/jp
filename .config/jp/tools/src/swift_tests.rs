use jp_tool::{Action, Context};
use pretty_assertions::assert_eq;

use super::*;
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput};

fn ctx() -> Context {
    Context {
        root: "/repo".into(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    }
}

/// Both generated inputs are built, in the order the Xcode build needs them:
/// the header must exist before the project is asked to scan it.
#[test]
fn prepare_builds_the_library_then_the_project() {
    let runner = MockProcessRunner::builder()
        .expect("just")
        .args(&["build-ffi", "debug"])
        .returns_success("")
        .expect("xcodegen")
        .args(&[
            "generate",
            "--spec",
            "apps/macos/project.yml",
            "--project",
            "apps/macos",
        ])
        .returns_success("");

    assert_eq!(prepare(&ctx(), "debug", &runner).unwrap(), None);
}

/// A failed library build short-circuits: the mock would panic on drop if
/// xcodegen had been expected and never run, which is what proves the ordering.
#[test]
fn prepare_stops_at_the_first_failure() {
    let runner = MockProcessRunner::builder()
        .expect("just")
        .returns_error("boom");

    let failure = prepare(&ctx(), "debug", &runner).unwrap();

    assert_eq!(
        failure,
        Some("Building `jp_ffi` failed:\n\n```\nboom\n```".to_owned())
    );
}

#[test]
fn report_prefers_stdout() {
    let output = ProcessOutput {
        stdout: "from stdout".to_owned(),
        stderr: "from stderr".to_owned(),
        status: ExitCode::from_code(1),
    };

    assert_eq!(report(&output, "tool"), "from stdout");
}

/// Most tools report on stderr; only `xcodebuild` puts diagnostics on stdout.
#[test]
fn report_falls_back_to_stderr() {
    let output = ProcessOutput {
        stdout: String::new(),
        stderr: "from stderr".to_owned(),
        status: ExitCode::from_code(1),
    };

    assert_eq!(report(&output, "tool"), "from stderr");
}

/// Silence plus a non-zero status is still worth reporting, so the caller is
/// not left with an empty code block.
#[test]
fn report_names_the_program_when_it_said_nothing() {
    let output = ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        status: ExitCode::from_code(70),
    };

    assert_eq!(
        report(&output, "xcodebuild"),
        "xcodebuild exited with status 70 and no diagnostics."
    );
}
