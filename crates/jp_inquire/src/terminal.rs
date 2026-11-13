//! Terminal restoration for prompts abandoned on background threads.
//!
//! Interactive widgets enable raw mode while waiting for input and restore it
//! when they return.
//! Because `inquire` and `reedline` block in synchronous reads until a key is
//! pressed, a process exiting while a prompt is open on a background thread
//! must restore the terminal itself via [`release_terminal`].

use std::io::{self, Write};

use crossterm::terminal;

use crate::inline_reply;

/// Escape sequence to disable bracketed paste mode (enabled by `reedline`).
const DISABLE_BRACKETED_PASTE: &str = "\x1b[?2004l";

/// Escape sequence to pop kitty keyboard enhancement flags (pushed by
/// `reedline`).
///
/// Ignored by terminals that do not support the kitty keyboard protocol.
const POP_KEYBOARD_ENHANCEMENT: &str = "\x1b[<1u";

/// Escape sequence to reset all SGR text attributes.
///
/// Prevents [`LEAVE_WIDGET`]'s erase-below sequence from filling with an active
/// shaded region's background color.
const RESET_ATTRIBUTES: &str = "\x1b[0m";

/// Escape sequence to show the cursor (hidden by `inquire` select menus).
const SHOW_CURSOR: &str = "\x1b[?25h";

/// Moves the cursor to the start of the next line and erases to the end of the
/// screen.
pub(crate) const LEAVE_WIDGET: &str = "\r\n\x1b[J";

/// Return `true` if the terminal currently has raw mode enabled.
///
/// Returns `false` if no controlling terminal is available or its mode cannot
/// be queried.
#[must_use]
pub fn terminal_held() -> bool {
    terminal::is_raw_mode_enabled().unwrap_or(false)
}

/// Restore terminal state while an unanswered prompt remains open on a
/// background thread.
///
/// Disables raw mode, resets terminal modes and text attributes on `out`, shows
/// the cursor, and moves to a clean line below the prompt.
/// Intended for process exit paths where the prompt thread is abandoned.
///
/// # Errors
///
/// Returns an error if raw mode cannot be disabled or writing to `out` fails.
pub fn release_terminal(out: &mut dyn Write) -> io::Result<()> {
    terminal::disable_raw_mode()?;

    out.write_all(release_sequence(inline_reply::is_open()).as_bytes())?;
    out.flush()
}

/// Build the escape sequence written by [`release_terminal`].
fn release_sequence(reply_open: bool) -> String {
    let mut sequence = String::new();
    if reply_open {
        sequence.push_str(DISABLE_BRACKETED_PASTE);
        sequence.push_str(POP_KEYBOARD_ENHANCEMENT);
    }
    sequence.push_str(RESET_ATTRIBUTES);
    sequence.push_str(SHOW_CURSOR);
    sequence.push_str(LEAVE_WIDGET);
    sequence
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
