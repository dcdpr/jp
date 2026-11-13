//! The inline reply widget, drawing a seeded buffer into a real terminal.
//!
//! The probe seeds the buffer with a cursor-up, a line erase, and a window
//! title.
//! Painted as they are, the first two wipe the row above the prompt.
//!
//! Unix only: the test answers reedline's cursor-position query by typing the
//! reply, and on Windows reedline asks the console instead, so the typed reply
//! would land in the buffer.
#![cfg(unix)]

use std::{fs, time::Duration};

use camino_tempfile::tempdir;
use jp_pty::{CommandBuilder, Size, Terminal};

/// The probe binary, built by cargo alongside this test.
const PROBE: &str = env!("CARGO_BIN_EXE_inline_reply_probe");

/// The probe that opens a reply and closes it when told to.
const CLOSING_PROBE: &str = env!("CARGO_BIN_EXE_prompt_probe");

/// How long a step waits on the child.
///
/// Longer than the harness default: this waits on process startup under
/// whatever else CI is running at the time.
const TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn a_seeded_buffer_is_shown_without_redrawing_the_screen() {
    let terminal = Terminal::pty(Size::new(10, 80))
        .expect("a pty")
        .with_timeout(TIMEOUT);
    let mut probe = terminal
        .spawn(CommandBuilder::new(PROBE))
        .expect("the probe to start");

    terminal
        .wait_for("the probe's rows", |screen| screen.row(1) == "above two")
        .expect("the probe to print its rows");

    // A terminal answers reedline's question about where the cursor is; this
    // one leaves that to the test. The probe printed two rows, so the cursor is
    // at the start of the third.
    terminal.send("\x1b[3;1R").expect("the answer to be typed");

    let screen = terminal
        .wait_for("the prompt", |screen| screen.contains(" end"))
        .expect("the prompt to be drawn");

    assert_eq!(
        screen.used(),
        [
            "above one",
            "above two",
            "Edit: result ?[1A?[2Kmid?]0;pwned? end"
        ],
        "{screen}"
    );

    terminal.send("\r").expect("Enter to be typed");

    let screen = terminal
        .wait_for("the outcome", |screen| screen.contains("outcome:"))
        .expect("the reply to be submitted");

    // The reply is the text the buffer was seeded with, escape sequences and
    // all: only what the widget showed was changed.
    assert_eq!(
        screen.row(3),
        r#"outcome: Submit("result \u{1b}[1A\u{1b}[2Kmid\u{1b}]0;pwned\u{7} end")"#,
        "{screen}"
    );

    // `"outcome:"` appears before `main` returns. Wait for the process to exit
    // so `Child::drop` does not kill it while `atexit` flushes coverage data.
    assert_eq!(
        probe.wait_within(TIMEOUT).expect("the exit check"),
        Some(true),
        "the probe exits successfully once the reply is submitted"
    );
}

#[test]
fn a_closed_reply_returns_without_being_submitted() {
    let tmp = tempdir().expect("a temporary directory");
    let close = tmp.path().join("close");

    let terminal = Terminal::pty(Size::new(10, 80))
        .expect("a pty")
        .with_timeout(TIMEOUT);
    let mut command = CommandBuilder::new(CLOSING_PROBE);
    command.args(["reply", close.as_str()]);
    let mut probe = terminal.spawn(command).expect("the probe to start");

    terminal
        .wait_for("the probe's row", |screen| screen.row(0) == "above")
        .expect("the probe to print its row");

    // The probe printed one row, so the cursor is at the start of the second.
    terminal.send("\x1b[2;1R").expect("the answer to be typed");
    terminal
        .wait_for("the prompt", |screen| screen.contains("Reply:"))
        .expect("the prompt to be drawn");

    terminal.send("half a").expect("the reply to be typed");
    terminal
        .wait_for("the typed reply", |screen| screen.contains("half a"))
        .expect("the reply to be drawn");

    fs::write(&close, "").expect("the close file to be written");

    let screen = terminal
        .wait_for("the outcome", |screen| screen.contains("outcome:"))
        .expect("the reply to return");

    assert_eq!(
        screen.used(),
        ["above", "Reply: half a", "outcome: Ok(Cancelled)"],
        "{screen}"
    );

    assert_eq!(
        probe.wait_within(TIMEOUT).expect("the exit check"),
        Some(true),
        "the probe exits once its reply has returned"
    );
}
