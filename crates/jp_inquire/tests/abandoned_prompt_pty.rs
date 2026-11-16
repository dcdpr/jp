//! PTY integration tests for [`jp_inquire::release_terminal`] when a prompt is
//! abandoned on a background thread.
#![cfg(unix)]

use std::{fs, time::Duration};

use camino_tempfile::tempdir;
use jp_pty::{CommandBuilder, Size, Terminal};

const PROBE: &str = env!("CARGO_BIN_EXE_abandoned_prompt_probe");
const TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn an_unanswered_select_gives_the_terminal_back() {
    let tmp = tempdir().expect("a temporary directory");
    let go = tmp.path().join("go");

    let terminal = Terminal::pty(Size::new(10, 80))
        .expect("a pty")
        .with_timeout(TIMEOUT);
    let mut command = CommandBuilder::new(PROBE);
    command.args(["select", go.as_str()]);
    let mut probe = terminal.spawn(command).expect("the probe to start");

    terminal
        .wait_for("the prompt", |screen| screen.contains("Run it?"))
        .expect("the prompt to be drawn");

    fs::write(&go, "").expect("the go file to be written");

    let screen = terminal
        .wait_for("the release", |screen| screen.contains("held after:"))
        .expect("the terminal to be released");

    // The probe never enters raw mode itself here, so the terminal was held by
    // the select alone. `held after` starts its row: the line feed before it
    // returned the carriage, which a terminal still in raw mode does not do.
    assert_eq!(
        screen.used(),
        [
            "above",
            "? Run it? [y,n,?]",
            "held before: true",
            r#"wrote: "\u{1b}[0m\u{1b}[?25h\r\n\u{1b}[J""#,
            "held after: false"
        ],
        "{screen}"
    );

    assert_eq!(
        probe.wait_within(TIMEOUT).expect("the exit check"),
        Some(true),
        "the probe exits with its prompt unanswered"
    );
}

#[test]
fn an_unanswered_reply_gives_the_terminal_back() {
    let tmp = tempdir().expect("a temporary directory");
    let go = tmp.path().join("go");

    let terminal = Terminal::pty(Size::new(10, 80))
        .expect("a pty")
        .with_timeout(TIMEOUT);
    let mut command = CommandBuilder::new(PROBE);
    command.args(["reply", go.as_str()]);
    let mut probe = terminal.spawn(command).expect("the probe to start");

    terminal
        .wait_for("the probe's row", |screen| screen.row(0) == "above")
        .expect("the probe to print its row");

    // A terminal answers reedline's question about where the cursor is; this
    // one leaves that to the test. The probe printed one row, so the cursor is
    // at the start of the second.
    terminal.send("\x1b[2;1R").expect("the answer to be typed");

    terminal
        .wait_for("the prompt", |screen| screen.contains("Reply:"))
        .expect("the prompt to be drawn");

    fs::write(&go, "").expect("the go file to be written");

    let screen = terminal
        .wait_for("the release", |screen| screen.contains("held after:"))
        .expect("the terminal to be released");

    // Bracketed paste and the keyboard enhancement flags are reedline's, so
    // they are undone only because a reply was open.
    assert_eq!(
        screen.used(),
        [
            "above",
            "Reply:",
            "held before: true",
            r#"wrote: "\u{1b}[?2004l\u{1b}[<1u\u{1b}[0m\u{1b}[?25h\r\n\u{1b}[J""#,
            "held after: false"
        ],
        "{screen}"
    );

    assert_eq!(
        probe.wait_within(TIMEOUT).expect("the exit check"),
        Some(true),
        "the probe exits with its prompt unanswered"
    );
}
