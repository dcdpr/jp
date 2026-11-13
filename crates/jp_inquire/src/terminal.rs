//! Giving the terminal back on behalf of a widget that cannot.
//!
//! A widget puts the terminal in raw mode for as long as it waits for a key,
//! and restores it when it returns.
//! Nothing can make a waiting widget return: `inquire` and `reedline` both
//! block in a read that only input ends.
//! A program that has to exit while a widget waits on another thread leaves
//! that thread behind, and calls [`release_terminal`] first so the terminal it
//! hands back is usable.

use std::io::{self, Write};

use crossterm::terminal;

use crate::inline_reply;

/// Turns bracketed paste off, which reedline turns on for a reply.
const DISABLE_BRACKETED_PASTE: &str = "\x1b[?2004l";

/// Pops the keyboard enhancement flags reedline pushes for a reply.
///
/// A terminal without the kitty keyboard protocol ignores it, and reedline
/// pushes nothing there.
const POP_KEYBOARD_ENHANCEMENT: &str = "\x1b[<1u";

/// Resets every text attribute.
///
/// A widget drawn inside a shaded region waits with the region's background
/// still active, and the erase in [`LEAVE_WIDGET`] fills with the active
/// background.
const RESET_ATTRIBUTES: &str = "\x1b[0m";

/// Shows the cursor, which a select menu hides.
const SHOW_CURSOR: &str = "\x1b[?25h";

/// Moves to the start of the next row and erases from there down, so output
/// that follows starts on a row of its own with none of the widget's rows below
/// it.
pub(crate) const LEAVE_WIDGET: &str = "\r\n\x1b[J";

/// Whether the terminal is in raw mode, which a widget enters for as long as it
/// waits for a key.
///
/// `false` when the terminal's mode cannot be read, which is the case without a
/// terminal.
#[must_use]
pub fn terminal_held() -> bool {
    terminal::is_raw_mode_enabled().unwrap_or(false)
}

/// Put the terminal back the way the open widgets found it, leaving them open.
///
/// Leaves raw mode, undoes the modes and styling the widgets set, and moves the
/// cursor to a row below what they drew.
/// `out` is the stream the widgets draw on.
///
/// The widgets keep waiting for input and are not told.
/// One that returns afterwards redraws on a terminal it no longer owns, so this
/// is for a program about to exit.
///
/// # Errors
///
/// Returns an error when the terminal's mode cannot be restored, or `out`
/// cannot be written to.
pub fn release_terminal(out: &mut dyn Write) -> io::Result<()> {
    terminal::disable_raw_mode()?;

    out.write_all(release_sequence(inline_reply::is_open()).as_bytes())?;
    out.flush()
}

/// What [`release_terminal`] writes: the reply widget's modes when
/// `reply_open`, then what every widget needs.
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
