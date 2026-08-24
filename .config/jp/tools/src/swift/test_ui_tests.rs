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
    let runner = prepared().expect("xcodebuild").returns_success(
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
    let runner = prepared()
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
    let runner = prepared().expect("xcodebuild").returns_success("");

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
///
/// Checked against the value rather than by setting `CI`: every test that runs
/// the tool reaches `under_ci`, so writing the variable here would race them.
#[test]
fn ci_is_the_variable_set_to_something() {
    assert!(is_ci(Some("1")));
    assert!(is_ci(Some("true")));
}

/// The app's own Xcode scheme sets `CI` to an empty string, which must not read
/// as being under CI.
#[test]
fn an_empty_ci_variable_is_not_being_under_ci() {
    assert!(!is_ci(Some("")));
}

#[test]
fn an_unset_ci_variable_is_not_being_under_ci() {
    assert!(!is_ci(None));
}

/// The whole point of the check: `xcodebuild` unions the selectors and ignores
/// one that matches nothing, so without this a typo alongside a real name is a
/// passing run that quietly did half the work.
#[test]
fn names_the_requested_tests_that_never_ran() {
    let executed = ["UISuite/ConversationListTests/clickSelects()".to_owned()];
    let requested = [
        "UISuite/ConversationListTests/clickSelects()".to_owned(),
        "UISuite/ConversationListTests/clickSelcts()".to_owned(),
    ];

    assert_eq!(unmatched(&requested, &executed), vec![
        "UISuite/ConversationListTests/clickSelcts()".to_owned()
    ]);
}

/// A suite stands for everything under it, so naming one is answered by any
/// test beneath it rather than by an identifier equal to it.
#[test]
fn a_suite_is_matched_by_the_tests_inside_it() {
    let executed = [
        "UISuite/ConversationListTests/clickSelects()".to_owned(),
        "UISuite/ConversationListTests/labelsRows()".to_owned(),
    ];

    assert!(unmatched(&["UISuite/ConversationListTests".to_owned()], &executed).is_empty());
    assert!(unmatched(&["UISuite".to_owned()], &executed).is_empty());
}

/// A prefix that stops mid-name is not a suite of that name.
/// Matching on the bare string would let `ConversationList` answer for
/// `ConversationListTests`.
#[test]
fn a_partial_name_is_not_a_match() {
    let executed = ["UISuite/ConversationListTests/clickSelects()".to_owned()];

    assert_eq!(
        unmatched(&["UISuite/ConversationList".to_owned()], &executed),
        vec!["UISuite/ConversationList".to_owned()]
    );
}

/// An unreadable bundle cannot answer the question, and answering "none of them
/// ran" would fail a suite that passed.
#[test]
fn nothing_is_unmatched_when_nothing_is_known() {
    let requested = ["UISuite/ConversationListTests/clickSelects()".to_owned()];

    assert!(unmatched(&requested, &[]).is_empty());
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
