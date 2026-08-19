use jp_tool::{Action, Context};
use pretty_assertions::assert_eq;

use super::{super::error_message, *};
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput};

/// The tools pass `ctx.root` straight to the runner and never touch the
/// filesystem, so a fixed path is enough.
fn ctx() -> Context {
    Context {
        root: "/repo".into(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    }
}

/// Expect the two preparation steps every build runs first.
fn prepared(profile: &str) -> MockProcessRunner {
    MockProcessRunner::builder()
        .expect("just")
        .args(&["build-ffi", profile])
        .returns_success("")
        .expect("xcodegen")
        .args(&[
            "generate",
            "--spec",
            "apps/macos/project.yml",
            "--project",
            "apps/macos",
        ])
        .returns_success("")
}

#[test]
fn builds_the_debug_configuration_by_default() {
    let runner = prepared("debug")
        .expect("xcodebuild")
        .args(&[
            "build",
            "-project",
            "apps/macos/JP.xcodeproj",
            "-scheme",
            "JP",
            "-configuration",
            "Debug",
            "-destination",
            "platform=macOS",
            "CODE_SIGNING_ALLOWED=NO",
            "-quiet",
        ])
        .returns_success("");

    let result = swift_check_impl(&ctx(), None, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "Build succeeded. No warnings or errors found."
    );
}

/// A Release build links the library from cargo's `release` directory, so the
/// preparation step has to build that profile rather than the default.
#[test]
fn builds_the_release_library_for_a_release_build() {
    let runner = prepared("release")
        .expect("xcodebuild")
        .args(&[
            "build",
            "-project",
            "apps/macos/JP.xcodeproj",
            "-scheme",
            "JP",
            "-configuration",
            "Release",
            "-destination",
            "platform=macOS",
            "CODE_SIGNING_ALLOWED=NO",
            "-quiet",
        ])
        .returns_success("");

    let result = swift_check_impl(&ctx(), Some("Release"), &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "Build succeeded. No warnings or errors found."
    );
}

/// Diagnostics from a passing build still reach the caller: the project treats
/// warnings as errors, so anything reported here is worth reading.
#[test]
fn reports_diagnostics_from_a_passing_build() {
    let runner = prepared("debug")
        .expect("xcodebuild")
        .returns_success("note: some advice from the compiler");

    let result = swift_check_impl(&ctx(), None, &runner).unwrap();

    assert_eq!(
        result.unwrap_content(),
        "```\nnote: some advice from the compiler\n```\n"
    );
}

/// `xcodebuild` reports compiler diagnostics on stdout, not stderr.
#[test]
fn reports_compiler_errors_from_stdout() {
    let runner = prepared("debug")
        .expect("xcodebuild")
        .returns(ProcessOutput {
            stdout: "WorkspaceReader.swift:12:5: error: cannot find 'nope' in scope".to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(65),
        });

    let message = error_message(swift_check_impl(&ctx(), None, &runner).unwrap());

    assert_eq!(
        message,
        "Swift build failed:\n\n```\nWorkspaceReader.swift:12:5: error: cannot find 'nope' in \
         scope\n```"
    );
}

/// A non-zero exit with nothing on either stream still has to say something.
#[test]
fn reports_a_bare_failure() {
    let runner = prepared("debug")
        .expect("xcodebuild")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            status: ExitCode::from_code(70),
        });

    let message = error_message(swift_check_impl(&ctx(), None, &runner).unwrap());

    assert_eq!(
        message,
        "Swift build failed:\n\n```\nxcodebuild exited with status 70 and no diagnostics.\n```"
    );
}

/// A failure building the library stops before `xcodebuild`, which would
/// otherwise fail on a missing header and bury the real cause.
#[test]
fn stops_when_the_library_fails_to_build() {
    let runner = MockProcessRunner::builder()
        .expect("just")
        .returns_error("error[E0425]: cannot find value `nope` in this scope");

    let message = error_message(swift_check_impl(&ctx(), None, &runner).unwrap());

    assert_eq!(
        message,
        "Building `jp_ffi` failed:\n\n```\nerror[E0425]: cannot find value `nope` in this \
         scope\n```"
    );
}

/// xcodegen is the one tool here that is not part of the Swift toolchain, so
/// its absence gets an install hint.
#[test]
fn points_at_homebrew_when_xcodegen_is_missing() {
    let runner = MockProcessRunner::builder()
        .expect("just")
        .returns_success("")
        .expect("xcodegen")
        .returns_error("command not found: xcodegen");

    let message = error_message(swift_check_impl(&ctx(), None, &runner).unwrap());

    assert!(message.contains("brew install xcodegen"), "got: {message}");
}
