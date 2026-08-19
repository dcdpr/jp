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

/// A swift-testing summary line, verbatim from a real run.
const PASSING_SUMMARY: &str =
    "\u{1005db} Test run with 2 tests in 2 suites passed after 11.2 seconds.";

const CLEANED_SUMMARY: &str = "Test run with 2 tests in 2 suites passed after 11.2 seconds.";

/// Expect the two preparation steps every Xcode run performs first.
fn prepared() -> MockProcessRunner {
    MockProcessRunner::builder()
        .expect("just")
        .args(&["build-ffi", "debug"])
        .returns_success("")
        .expect("xcodegen")
        .returns_success("")
}

#[test]
fn runs_each_named_test() {
    let runner = prepared()
        .expect("xcodebuild")
        .args(&[
            "test",
            "-project",
            "apps/macos/JP.xcodeproj",
            "-scheme",
            "JP",
            "-destination",
            "platform=macOS",
            "-resultBundlePath",
            "tmp/uitests/run.xcresult",
            "-only-testing:JPUITests/UISuite/ConversationListTests/clickSelects()",
            "-only-testing:JPUITests/UISuite/ConversationListTests/labelsRows()",
        ])
        .returns_success(PASSING_SUMMARY);

    let tests = [
        "UISuite/ConversationListTests/clickSelects()".to_owned(),
        "UISuite/ConversationListTests/labelsRows()".to_owned(),
    ];
    let result = swift_test_ui_impl(&ctx(), Some(&tests), &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        format!("```\nJP: {CLEANED_SUMMARY}\n```")
    );
}

/// The whole point of the tool being separate: there is no way to ask it for
/// every UI test, because every one of them launches the app.
#[test]
fn refuses_to_run_without_names() {
    let runner = MockProcessRunner::builder()
        .expect("xcodebuild")
        .returns_success(
            r#"{"values":[{"enabledTests":[
                {"identifier":"JPUITests/UISuite/ConversationListTests/labelsRows()"},
                {"identifier":"JPTests/ConversationRefTests/exportsAsAURI()"}
            ]}]}"#,
        );

    let message = error_message(swift_test_ui_impl(&ctx(), None, &runner).unwrap());

    assert!(message.contains("`tests` is required"), "got: {message}");
    assert!(message.contains("just test-app-ui"), "got: {message}");

    // Asked of the bundle, so the list cannot drift from what is really there.
    // Named as this tool takes them: no bundle prefix, because it adds one.
    assert!(
        message.contains("\nUISuite/ConversationListTests/labelsRows()"),
        "got: {message}"
    );

    // `-enumerate-tests` reports the whole scheme, and the unit tests are not
    // this tool's to run.
    assert!(!message.contains("ConversationRefTests"), "got: {message}");
}

/// The list is a nicety on a path that has already failed.
/// An `xcodebuild` too old to enumerate, or a shape this cannot read, costs the
/// caller the naming convention instead of a second failure.
#[test]
fn refuses_helpfully_when_enumeration_fails() {
    let runner = MockProcessRunner::builder()
        .expect("xcodebuild")
        .returns_error("unknown option -enumerate-tests");

    let message = error_message(swift_test_ui_impl(&ctx(), None, &runner).unwrap());

    assert!(message.contains("`tests` is required"), "got: {message}");
    assert!(message.contains("trailing `()`"), "got: {message}");
}

/// An empty list is the same request as no list, and gets the same answer
/// rather than an `xcodebuild` run with no filter — which would run the whole
/// bundle, the one outcome this tool exists to prevent.
#[test]
fn refuses_to_run_with_an_empty_list() {
    let runner = MockProcessRunner::builder()
        .expect("xcodebuild")
        .returns_success("");

    let message = error_message(swift_test_ui_impl(&ctx(), Some(&[]), &runner).unwrap());

    assert!(message.contains("`tests` is required"), "got: {message}");
}

/// A run is stopped from outside, so the marker that stops it has to be a line
/// `xcodebuild` really prints.
/// This is one, verbatim.
#[test]
fn recognizes_the_line_that_stops_a_run() {
    let line =
        "\u{1005db} Test \"selects a row\" recorded an issue at Foo.swift:12:5: Issue recorded";

    assert!(line.contains(FAILURE_MARKER));
}

/// CI has nobody waiting on it and wants every result from the one run it gets,
/// so it opts out of stopping.
#[test]
fn ci_is_read_from_the_environment() {
    // SAFETY: this test reads back only what it just wrote, and the tools crate
    // runs its tests in one process where nothing else touches `CI`.
    unsafe {
        std::env::set_var("CI", "1");
    }
    assert!(under_ci());

    unsafe {
        std::env::set_var("CI", "");
    }
    assert!(!under_ci(), "an empty value is not being under CI");

    unsafe {
        std::env::remove_var("CI");
    }
    assert!(!under_ci());
}

/// A failing run names the screenshots the tests left behind, so what was on
/// screen can be looked at rather than guessed.
#[test]
fn reports_a_failure_with_its_summary() {
    let runner = prepared().expect("xcodebuild").returns(ProcessOutput {
        stdout: "\u{1005db} Test \"selects a row\" recorded an issue at Foo.swift:12:5: Issue \
                 recorded\n"
            .to_owned(),
        stderr: String::new(),
        status: ExitCode::from_code(1),
    });

    let tests = ["UISuite/ConversationListTests/clickSelects()".to_owned()];
    let message = error_message(swift_test_ui_impl(&ctx(), Some(&tests), &runner).unwrap());

    assert!(message.contains("recorded an issue"), "got: {message}");
}
