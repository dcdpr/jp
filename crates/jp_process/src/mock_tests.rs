use std::{sync::Mutex as StdMutex, time::Duration};

use camino::Utf8Path;

use super::*;

fn dir() -> &'static Utf8Path {
    Utf8Path::new("/repo")
}

#[test]
fn expectations_are_answered_in_order() {
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["status"])
        .returns_success("clean")
        .expect("git")
        .returns_error("no such revision");

    let first = runner.run("git", &["status"], dir()).unwrap();
    let second = runner.run("git", &["show", "nope"], dir()).unwrap();

    assert_eq!(first.stdout, "clean");
    assert!(first.success());
    assert_eq!(second.stderr, "no such revision");
    assert_eq!(second.status, ExitCode::from_code(1));
}

#[test]
fn a_command_other_than_the_one_expected_is_an_error() {
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["status"])
        .returns_success("");

    let program = runner.run("cargo", &["status"], dir()).unwrap_err();
    assert_eq!(
        program.to_string(),
        "Expected program 'git' but got 'cargo'"
    );

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["status"])
        .returns_success("");

    let args = runner.run("git", &["log"], dir()).unwrap_err();
    assert_eq!(
        args.to_string(),
        "Expected args [\"status\"] but got [\"log\"]"
    );
}

#[test]
fn a_command_beyond_the_expected_ones_is_an_error() {
    let runner = MockProcessRunner::never_called();

    let error = runner.run("git", &["status"], dir()).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Unexpected command: git status (no more expectations)"
    );
}

#[test]
fn a_program_that_is_not_installed_fails_to_spawn() {
    let runner = MockProcessRunner::builder()
        .expect("xcrun")
        .fails_to_spawn();

    let error = runner.run("xcrun", &[], dir()).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
#[should_panic(expected = "MockProcessRunner dropped with 1 unfulfilled expectation(s)")]
fn an_expected_command_that_never_ran_fails_the_test() {
    drop(MockProcessRunner::success("unused"));
}

#[test]
fn a_responding_mock_answers_from_the_command() {
    let runner = MockProcessRunner::responding(|spec| {
        Ok(ProcessOutput {
            stdout: spec.args.join(","),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    });

    assert_eq!(
        runner.run("echo", &["a", "b"], dir()).unwrap().stdout,
        "a,b"
    );
    assert_eq!(runner.run("echo", &["c"], dir()).unwrap().stdout, "c");
    assert_eq!(runner.calls().len(), 2);
}

fn scripted(stdout: &str, stderr: &str) -> MockProcessRunner {
    MockProcessRunner::builder()
        .expect_any()
        .returns(ProcessOutput {
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            status: ExitCode::success(),
        })
}

#[test]
fn scripted_stderr_reaches_the_line_sink() {
    let runner = scripted("out\n", "one\ntwo\n");
    let lines = Arc::new(StdMutex::new(Vec::new()));
    let sink = Arc::clone(&lines);

    let finished = runner
        .execute(&ProcessSpec::new("tool", ["run"], dir()), &Watch {
            stderr_lines: Some(Arc::new(move |line: &str| {
                sink.lock().unwrap().push(line.to_owned());
            })),
            ..Watch::default()
        })
        .unwrap();

    assert_eq!(*lines.lock().unwrap(), ["one", "two"]);
    assert_eq!(finished.ended, Ended::Exited);
}

#[test]
fn a_scripted_line_matching_the_stop_ends_the_run_as_stopped() {
    let runner = scripted("building\n", "error: failed\n");

    let finished = runner
        .run_until(
            "xcodebuild",
            &["test"],
            dir(),
            Arc::new(|line: &str| line.starts_with("error:")),
            Duration::ZERO,
        )
        .unwrap();

    assert_eq!(finished.ended, Ended::Stopped);
}

#[test]
fn a_cancelled_run_ends_as_cancelled() {
    let runner = scripted("", "");
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let finished = runner
        .execute(&ProcessSpec::new("tool", ["run"], dir()), &Watch {
            cancellation: Some(cancellation),
            ..Watch::default()
        })
        .unwrap();

    assert_eq!(finished.ended, Ended::Cancelled);
}
