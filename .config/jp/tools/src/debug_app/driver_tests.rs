use super::{failure, is_not_permitted, said};
use crate::util::runner::{ExitCode, ProcessOutput};

fn output(stdout: &str, stderr: &str) -> ProcessOutput {
    ProcessOutput {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        status: ExitCode::from_code(1),
    }
}

/// `just` puts only "recipe failed" on stderr and leaves the compiler
/// diagnostics on the recipe's stdout, so quoting one stream loses the reason.
#[test]
fn a_failure_quotes_both_streams() {
    assert_eq!(
        said(&output(
            "error: cannot find 'Slot'\n",
            "recipe failed with exit code 1\n"
        )),
        "error: cannot find 'Slot'\nrecipe failed with exit code 1"
    );
}

#[test]
fn a_failure_quotes_whichever_stream_spoke() {
    assert_eq!(said(&output("only stdout\n", "")), "only stdout");
    assert_eq!(said(&output("", "only stderr\n")), "only stderr");
}

/// A command that failed silently still has to report something, or the message
/// reads as an empty code block.
#[test]
fn a_silent_failure_says_so() {
    assert_eq!(
        said(&output("", "  \n")),
        "(it said nothing on either stream)"
    );
}

#[test]
fn failure_reports_the_command_the_kind_the_message_and_the_hint() {
    let stdout = r#"{"error": {"kind": "not_permitted", "message": "not trusted to read another application's accessibility tree", "hint": "grant Accessibility to the terminal"}}"#;

    assert_eq!(
        failure("tree", stdout, ""),
        "`jpdrive tree` failed (not_permitted): not trusted to read another application's \
         accessibility tree\n\nHint: grant Accessibility to the terminal"
    );
}

#[test]
fn failure_omits_an_absent_hint() {
    let stdout = r#"{"error": {"kind": "app_not_running", "message": "no process is running under pid 4321"}}"#;

    assert_eq!(
        failure("act", stdout, ""),
        "`jpdrive act` failed (app_not_running): no process is running under pid 4321"
    );
}

/// A driver that died before writing JSON is exactly when the raw streams
/// matter.
#[test]
fn failure_falls_back_to_the_raw_streams() {
    assert_eq!(
        failure("tree", "", "dyld: Library not loaded\n"),
        "`jpdrive tree` failed and reported nothing \
         parseable.\n\nstdout:\n\n```\n\n```\n\nstderr:\n\n```\ndyld: Library not loaded\n```"
    );
}

/// Only a refused read is worth a follow-up probe.
/// Any other failure would run the diagnostic for nothing and bury the real
/// error under it.
#[test]
fn only_a_refusal_asks_for_a_diagnosis() {
    let refused = r#"{"error": {"kind": "not_permitted", "message": "not trusted"}}"#;
    let missing = r#"{"error": {"kind": "app_not_running", "message": "no process"}}"#;

    assert!(is_not_permitted(refused));
    assert!(!is_not_permitted(missing));
    assert!(!is_not_permitted("dyld: Library not loaded"));
}
