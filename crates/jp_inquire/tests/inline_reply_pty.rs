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

use std::time::Duration;

use jp_pty::{CommandBuilder, Size, Terminal};

/// The probe binary, built by cargo alongside this test.
const PROBE: &str = env!("CARGO_BIN_EXE_inline_reply_probe");

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
    let _probe = terminal
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
}
