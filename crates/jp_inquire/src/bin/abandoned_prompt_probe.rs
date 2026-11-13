//! A program a pty test spawns to leave a prompt unanswered.
//!
//! It opens the widget its first argument names on a thread of its own, waits
//! for the file its second argument names to appear, gives the terminal back,
//! and exits with the widget still waiting for a key.
//!
//! For a reply, the terminal is put in raw mode before anything is printed, so
//! the test can answer reedline's cursor-position query as soon as the first
//! row appears: typed earlier, the answer would be echoed onto the screen.
//! A select is left to enter raw mode itself.
//!
//! The probe prints whether the terminal is held before it gives it back, and
//! afterwards what [`release_terminal`] wrote, escaped, and whether the
//! terminal is still held.
//! The rows printed afterwards end in a bare line feed, which returns the
//! carriage only on a terminal that has left raw mode.

use std::{
    env,
    io::{self, Write as _},
    path::PathBuf,
    thread,
    time::Duration,
};

use crossterm::terminal;
use jp_inquire::{InlineOption, InlineReply, InlineSelect, release_terminal, terminal_held};

/// How often the probe looks for the file that tells it to go on.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn main() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let widget = args.next().ok_or_else(|| io::Error::other("no widget"))?;
    let go = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("no file to wait for"))?;

    let reply = widget == "reply";
    if reply {
        terminal::enable_raw_mode()?;
    }

    let mut stdout = io::stdout();
    write!(stdout, "above\r\n")?;
    stdout.flush()?;

    // Never joined: the widget waits for a key the test does not type.
    thread::spawn(move || {
        if reply {
            drop(InlineReply::new("Reply:").prompt(Box::new(io::stdout())));
        } else {
            let options = vec![
                InlineOption::new('y', "run it"),
                InlineOption::new('n', "skip it"),
            ];
            drop(InlineSelect::new("Run it?", options).prompt(&mut io::stdout()));
        }
    });

    while !go.exists() {
        thread::sleep(POLL_INTERVAL);
    }

    // On the row below the widget, which is where the release starts from.
    write!(stdout, "\r\nheld before: {}", terminal_held())?;

    let mut written = Vec::new();
    release_terminal(&mut written)?;
    stdout.write_all(&written)?;

    write!(
        stdout,
        "wrote: {:?}\nheld after: {}\n",
        String::from_utf8_lossy(&written),
        terminal_held()
    )?;
    stdout.flush()
}
