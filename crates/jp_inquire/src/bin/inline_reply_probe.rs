//! A program a pty test spawns to show an [`InlineReply`] seeded with text that
//! carries escape sequences, the way a tool result reaches the result editor.
//!
//! reedline asks the terminal where the cursor is before it draws, and the test
//! answers.
//! The terminal is put in raw mode before anything is printed, so the answer
//! can be typed as soon as the first rows appear: typed earlier, it would be
//! echoed onto the screen and held back until a line break.
//!
//! When the reply is submitted, the probe prints it, escaped, on the row below.

use std::io::{self, Write as _};

use crossterm::terminal;
use jp_inquire::InlineReply;

/// What the buffer is seeded with: a cursor-up and a line erase that would wipe
/// the row above the prompt, and a window title.
const SEED: &str = "result \x1b[1A\x1b[2Kmid\x1b]0;pwned\x07 end";

fn main() -> io::Result<()> {
    terminal::enable_raw_mode()?;

    let mut stdout = io::stdout();
    write!(stdout, "above one\r\nabove two\r\n")?;
    stdout.flush()?;

    let outcome = InlineReply::new("Edit:")
        .with_initial_text(SEED)
        .prompt(Box::new(io::stdout()))
        .map_err(|error| io::Error::other(error.to_string()))?;

    terminal::disable_raw_mode()?;
    writeln!(stdout, "outcome: {outcome:?}")?;
    stdout.flush()
}
