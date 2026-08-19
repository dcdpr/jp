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
///
/// The leading glyph is the SF Symbol swift-testing prefixes its output with.
const PASSING_SUMMARY: &str =
    "\u{1005db} Test run with 30 tests in 6 suites passed after 0.028 seconds.";

/// What the summary looks like once the tool has cleaned it up.
const CLEANED_SUMMARY: &str = "Test run with 30 tests in 6 suites passed after 0.028 seconds.";

/// The driver package's own summary, so a test can tell the two runs apart.
const DRIVE_SUMMARY: &str =
    "\u{1005db} Test run with 33 tests in 4 suites passed after 0.012 seconds.";

const CLEANED_DRIVE: &str = "Test run with 33 tests in 4 suites passed after 0.012 seconds.";

/// A bundle whose tests are all swift-testing always draws this from the
/// `XCTest` runner, whether or not anything ran.
const XCTEST_ZERO: &str =
    "Executed 0 tests, with 0 failures (0 unexpected) in 0.000 (0.000) seconds";

/// Expect the driver package run and the two app preparation steps, in the
/// order a full run performs them.
///
/// The package goes first because it is the cheap one, so a failure there is
/// reported before a minute of `xcodebuild`.
fn prepared() -> MockProcessRunner {
    MockProcessRunner::builder()
        .expect("swift")
        .args(&["test", "--package-path", "apps/macos/Tools/jpdrive"])
        .returns_success(DRIVE_SUMMARY)
        .expect("just")
        .args(&["build-ffi", "debug"])
        .returns_success("")
        .expect("xcodegen")
        .returns_success("")
}

/// Expect only the app preparation steps, for a run targeting the app alone.
fn prepared_app_only() -> MockProcessRunner {
    MockProcessRunner::builder()
        .expect("just")
        .args(&["build-ffi", "debug"])
        .returns_success("")
        .expect("xcodegen")
        .returns_success("")
}

/// The default run covers the driver package and the app's unit tests, and
/// never the UI bundle: the filter names `JPTests` even with nothing to narrow
/// to, because both bundles are in the one scheme.
#[test]
fn runs_both_targets_by_default() {
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
            "-only-testing:JPTests",
        ])
        .returns_success(PASSING_SUMMARY);

    let result = swift_test_impl(&ctx(), None, None, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        format!("```\nDriveKit: {CLEANED_DRIVE}\nJP: {CLEANED_SUMMARY}\n```")
    );
}

/// The driver package builds and runs without an Xcode project, so targeting it
/// must not drag the app's preparation steps along.
#[test]
fn the_drive_target_skips_the_app_entirely() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .args(&["test", "--package-path", "apps/macos/Tools/jpdrive"])
        .returns_success(DRIVE_SUMMARY);

    let result = swift_test_impl(&ctx(), None, Some("drive"), &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        format!("```\nDriveKit: {CLEANED_DRIVE}\n```")
    );
}

#[test]
fn the_app_target_runs_only_the_unit_bundle() {
    let runner = prepared_app_only()
        .expect("xcodebuild")
        .args(&[
            "test",
            "-project",
            "apps/macos/JP.xcodeproj",
            "-scheme",
            "JP",
            "-destination",
            "platform=macOS",
            "-only-testing:JPTests",
        ])
        .returns_success(PASSING_SUMMARY);

    let result = swift_test_impl(&ctx(), None, Some("app"), &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        format!("```\nJP: {CLEANED_SUMMARY}\n```")
    );
}

/// The UI tests are not reachable from this tool at all, and asking for them
/// says where they went rather than reporting an unknown target.
#[test]
fn the_ui_target_points_at_the_other_tool() {
    // Nothing may be spawned: a run that started building before refusing would
    // cost a minute to say no.
    let runner = MockProcessRunner::never_called();

    let message = error_message(swift_test_impl(&ctx(), None, Some("ui"), &runner).unwrap());

    assert!(message.contains("swift_test_ui"), "got: {message}");
}

/// swift-testing writes an issue's message on its own line, under a header
/// naming only the kind of issue.
/// Reporting the header alone turns every recorded issue into "Issue recorded"
/// and drops the sentence saying what went wrong.
#[test]
fn keeps_the_message_under_a_recorded_issue() {
    let runner = prepared_app_only()
        .expect("xcodebuild")
        .returns(ProcessOutput {
            stdout: "\u{1005db} Test \"selects a row\" recorded an issue at Foo.swift:12:5: Issue \
                     recorded\n\u{21b3} the transcript never appeared. On screen instead: \
                     /tmp/uitests/a.png\n"
                .to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let message = error_message(swift_test_impl(&ctx(), None, Some("app"), &runner).unwrap());

    assert!(message.contains("recorded an issue"), "got: {message}");
    assert!(
        message.contains("On screen instead: /tmp/uitests/a.png"),
        "got: {message}"
    );
}

#[test]
fn rejects_an_unknown_target() {
    let message = Target::parse(Some("both")).unwrap_err();

    assert_eq!(
        message,
        "unknown target 'both', expected one of: app, drive, all"
    );
}

/// An absent target runs everything, which is what makes the tool useful
/// without the caller knowing the project has two test suites in the first
/// place.
#[test]
fn an_absent_target_means_all() {
    assert_eq!(Target::parse(None).unwrap(), Target::All);
    assert_eq!(Target::parse(Some("all")).unwrap(), Target::All);
    assert_eq!(Target::parse(Some("app")).unwrap(), Target::App);
    assert_eq!(Target::parse(Some("drive")).unwrap(), Target::Drive);
}

/// Each runner takes its own filter syntax, so a name has to be translated for
/// whichever target it is addressed to.
#[test]
fn passes_a_filter_to_each_runner_in_its_own_form() {
    let drive = MockProcessRunner::builder()
        .expect("swift")
        .args(&[
            "test",
            "--package-path",
            "apps/macos/Tools/jpdrive",
            "--filter",
            "Tree",
        ])
        .returns_success(DRIVE_SUMMARY);

    swift_test_impl(&ctx(), Some("Tree"), Some("drive"), &drive).unwrap();

    let app = prepared_app_only()
        .expect("xcodebuild")
        .args(&[
            "test",
            "-project",
            "apps/macos/JP.xcodeproj",
            "-scheme",
            "JP",
            "-destination",
            "platform=macOS",
            "-only-testing:JPTests/WorkspaceReaderTests",
        ])
        .returns_success(PASSING_SUMMARY);

    swift_test_impl(&ctx(), Some("WorkspaceReaderTests"), Some("app"), &app).unwrap();
}

/// A filter naming a suite in the other target matches nothing there, which is
/// the mistake the error has to name rather than passing over.
#[test]
fn a_filter_matching_nothing_in_one_target_fails_the_run() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .returns_success("Test run with 0 tests passed after 0.001 seconds.");

    let message = error_message(
        swift_test_impl(&ctx(), Some("WorkspaceReaderTests"), Some("drive"), &runner).unwrap(),
    );

    assert!(message.contains("no test ran"), "got: {message}");
    assert!(
        message.contains("matches nothing in the bundle"),
        "got: {message}"
    );
}

/// The `XCTest` runner reports zero tests for a bundle that has none of its
/// own, which says nothing about whether the swift-testing tests ran.
/// Treating it as a result would make a mistyped filter look like a pass.
#[test]
fn an_xctest_zero_count_alone_is_not_a_pass() {
    let runner = prepared_app_only()
        .expect("xcodebuild")
        .returns_success(format!("{XCTEST_ZERO}\n** TEST SUCCEEDED **"));

    let message =
        error_message(swift_test_impl(&ctx(), Some("NoSuchSuite"), Some("app"), &runner).unwrap());

    assert!(message.contains("no test ran"), "got: {message}");
}

/// A real run prints both, and only the swift-testing line carries a count
/// worth reading.
#[test]
fn drops_the_xctest_zero_count_beside_a_real_summary() {
    let runner = prepared_app_only()
        .expect("xcodebuild")
        .returns_success(format!("{XCTEST_ZERO}\n{PASSING_SUMMARY}"));

    let result = swift_test_impl(&ctx(), None, Some("app"), &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        format!("```\nJP: {CLEANED_SUMMARY}\n```")
    );
}

/// A run that reported no summary is a run that did nothing.
/// Reporting it as a pass would make every filtered run worthless.
#[test]
fn a_run_with_no_summary_is_not_a_pass() {
    let runner = prepared_app_only()
        .expect("xcodebuild")
        .returns_success("Testing started\nnote: something unrelated");

    let message =
        error_message(swift_test_impl(&ctx(), Some("NoSuchSuite"), Some("app"), &runner).unwrap());

    assert!(message.contains("no test ran"), "got: {message}");
}

/// `XCTest` words its summary differently from swift-testing, and a bundle can
/// hold both.
#[test]
fn recognizes_an_xctest_summary() {
    let runner = prepared_app_only().expect("xcodebuild").returns_success(
        "Test Suite 'All tests' passed\n\t Executed 20 tests, with 0 failures in 1.234 seconds",
    );

    let result = swift_test_impl(&ctx(), None, Some("app"), &runner).unwrap();

    assert!(
        result.unwrap_content().contains("Executed 20 tests"),
        "expected the XCTest summary to be reported"
    );
}

/// The raw log is thousands of lines of compiler invocations; only the lines
/// that say what ran are worth relaying.
#[test]
fn drops_build_noise_from_the_summary() {
    let runner = prepared_app_only()
        .expect("xcodebuild")
        .returns_success(format!(
            "CompileSwift normal arm64 WorkspaceReader.swift\nLd \
             JP.app/Contents/MacOS/JP\n{PASSING_SUMMARY}"
        ));

    let result = swift_test_impl(&ctx(), None, Some("app"), &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        format!("```\nJP: {CLEANED_SUMMARY}\n```")
    );
}

#[test]
fn reports_failing_tests() {
    let runner = prepared_app_only()
        .expect("xcodebuild")
        .returns(ProcessOutput {
            stdout: "WorkspaceReaderTests.swift:61: error: Expectation failed".to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(65),
        });

    let message = error_message(swift_test_impl(&ctx(), None, Some("app"), &runner).unwrap());

    assert_eq!(
        message,
        "JP tests failed:\n\n```\nWorkspaceReaderTests.swift:61: error: Expectation failed\n```"
    );
}

/// A failing package run is labelled as such, so it is clear which suite broke
/// without reading the diagnostics.
#[test]
fn names_the_target_that_failed() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .returns(ProcessOutput {
            stdout: "ActTests.swift:32: error: Expectation failed".to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let message = error_message(swift_test_impl(&ctx(), None, Some("drive"), &runner).unwrap());

    assert!(
        message.starts_with("DriveKit tests failed:"),
        "got: {message}"
    );
}

/// A crashed run reports no failing test and no summary.
/// Naming the crash beats the alternative, which was dumping the whole build
/// log and leaving the reader to find the end of it.
#[test]
fn names_a_crash_rather_than_dumping_the_log() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .returns(ProcessOutput {
            stdout: "Building for debugging...\nRestarting after unexpected exit, crash, or test \
                     timeout in ScrollMemoryTests.roundTrips()"
                .to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let message = error_message(swift_test_impl(&ctx(), None, Some("drive"), &runner).unwrap());

    assert_eq!(
        message,
        "DriveKit tests failed:\n\n```\nRestarting after unexpected exit, crash, or test timeout \
         in ScrollMemoryTests.roundTrips()\n```"
    );
}

/// A run that died with nothing recognizable in it still has to say something,
/// and the useful end of a build log is the last of it, not the first.
#[test]
fn shows_the_end_of_an_unrecognizable_failure() {
    let noise = (0..100)
        .map(|index| format!("CompileSwift normal arm64 File{index}.swift"))
        .collect::<Vec<_>>()
        .join("\n");

    let runner = MockProcessRunner::builder()
        .expect("swift")
        .returns(ProcessOutput {
            stdout: format!("{noise}\nthe last thing it printed"),
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let message = error_message(swift_test_impl(&ctx(), None, Some("drive"), &runner).unwrap());

    assert!(
        message.contains("the run died rather than failing"),
        "got: {message}"
    );
    assert!(
        message.contains("the last thing it printed"),
        "got: {message}"
    );
    assert!(
        !message.contains("File0.swift"),
        "expected the end of the log, not the start of it"
    );
}

/// A failing run names the test that failed and then how the run ended, and
/// both are wanted: the first says what to fix, the second says how much broke.
#[test]
fn reports_the_failing_test_and_the_summary() {
    let runner = prepared_app_only()
        .expect("xcodebuild")
        .returns(ProcessOutput {
            stdout: "CompileSwift normal arm64\n\u{1005df} Test \"orders an empty list\" recorded \
                     an issue at TimestampTests.swift:88\n\u{1005db} Test run with 30 tests in 6 \
                     suites failed after 0.03 seconds."
                .to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(65),
        });

    let message = error_message(swift_test_impl(&ctx(), None, Some("app"), &runner).unwrap());

    assert_eq!(
        message,
        "JP tests failed:\n\n```\nTest \"orders an empty list\" recorded an issue at \
         TimestampTests.swift:88\nTest run with 30 tests in 6 suites failed after 0.03 \
         seconds.\n```"
    );
}

/// A failure in the cheap suite stops the run before the expensive one, so the
/// diagnostics arrive in seconds rather than after a full app build.
#[test]
fn a_package_failure_stops_before_the_app_build() {
    let runner = MockProcessRunner::builder()
        .expect("swift")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: no such module 'Testing'".to_owned(),
            status: ExitCode::from_code(1),
        });

    let message = error_message(swift_test_impl(&ctx(), None, None, &runner).unwrap());

    assert!(
        message.starts_with("DriveKit tests failed:"),
        "got: {message}"
    );
}

/// The bridging header has to exist before `xcodebuild` plans the build, so a
/// failed library build stops the run rather than producing a confusing "file
/// not found" from the header scan.
#[test]
fn stops_when_the_library_fails_to_build() {
    let runner = MockProcessRunner::builder()
        .expect("just")
        .returns_error("error: could not compile `jp_ffi`");

    let message = error_message(swift_test_impl(&ctx(), None, Some("app"), &runner).unwrap());

    assert!(
        message.starts_with("Building `jp_ffi` failed:"),
        "got: {message}"
    );
}
