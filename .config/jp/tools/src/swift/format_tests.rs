use jp_tool::{Action, Context};
use pretty_assertions::assert_eq;

use super::{super::error_message, *};
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

#[test]
fn rewrites_sources_in_place() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .args(&[
            "format",
            "--in-place",
            "--recursive",
            "--parallel",
            "apps/macos/Sources",
            "apps/macos/Tests",
            "apps/macos/UITests",
            "apps/macos/Tools/jpdrive/Sources",
            "apps/macos/Tools/jpdrive/Tests",
        ])
        .returns_success("");

    let result = swift_format_impl(&ctx(), false, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "Formatted apps/macos/Sources, apps/macos/Tests, apps/macos/UITests, \
         apps/macos/Tools/jpdrive/Sources, apps/macos/Tools/jpdrive/Tests."
    );
}

/// `swift format --in-place` says nothing at all, whether it rewrote every file
/// or none.
/// Claiming "no files to format" from that silence would be a guess, and a
/// wrong one whenever it did rewrite something.
#[test]
fn does_not_claim_nothing_changed() {
    let runner = MockProcessRunner::success("");

    let result = swift_format_impl(&ctx(), false, &runner).unwrap();

    assert!(
        !result.unwrap_content().contains("No files"),
        "the formatter cannot know whether anything changed, so it must not say"
    );
}

/// Anything the formatter does say is passed along, since it only speaks up
/// when something is wrong.
#[test]
fn relays_what_the_formatter_reported() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "JPApp.swift:3:1: warning: [Indentation] unexpected indentation".to_owned(),
            status: ExitCode::success(),
        });

    let result = swift_format_impl(&ctx(), false, &runner).unwrap();

    assert!(
        result.unwrap_content().contains("unexpected indentation"),
        "expected the formatter's own output to be relayed"
    );
}

#[test]
fn lints_without_rewriting_in_check_mode() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .args(&[
            "format",
            "lint",
            "--strict",
            "--recursive",
            "--parallel",
            "apps/macos/Sources",
            "apps/macos/Tests",
            "apps/macos/UITests",
            "apps/macos/Tools/jpdrive/Sources",
            "apps/macos/Tools/jpdrive/Tests",
        ])
        .returns_success("");

    let result = swift_format_impl(&ctx(), true, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "Swift sources are correctly formatted."
    );
}

#[test]
fn reports_lint_violations_in_check_mode() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "JPApp.swift:3:1: warning: [NeverForceUnwrap] do not force unwrap".to_owned(),
            status: ExitCode::from_code(1),
        });

    let message = error_message(swift_format_impl(&ctx(), true, &runner).unwrap());

    assert_eq!(
        message,
        "Swift formatting or lint violations:\n\n```\nJPApp.swift:3:1: warning: \
         [NeverForceUnwrap] do not force unwrap\n```"
    );
}

/// A clean exit with findings on stderr is still a violation: `--strict` makes
/// the findings meaningful, and trusting the exit status alone would hide them.
#[test]
fn treats_findings_as_violations_even_on_a_clean_exit() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "JPApp.swift:3:1: warning: [Indentation] unexpected indentation".to_owned(),
            status: ExitCode::success(),
        });

    let message = error_message(swift_format_impl(&ctx(), true, &runner).unwrap());

    assert!(message.contains("Indentation"), "got: {message}");
}

#[test]
fn reports_a_formatter_failure() {
    let runner = MockProcessRunner::error("error: unknown option '--nope'");

    let message = error_message(swift_format_impl(&ctx(), false, &runner).unwrap());

    assert_eq!(
        message,
        "swift format failed: error: unknown option '--nope'"
    );
}
