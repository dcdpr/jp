//! Test probe for `abandoned_prompt_pty` that exits while a prompt is still
//! open on a background thread.
//!
//! Spawns the widget named by the first argument (`reply` or `select`) on a
//! background thread, waits for the marker file named by the second argument to
//! appear, calls [`release_terminal`], and prints `terminal_held()` before and
//! after the release along with the escaped output of `release_terminal`.
//!
//! For `reply`, raw mode is enabled before printing so the PTY test can send
//! reedline's cursor-position response without echoing it.

use std::{
    env,
    io::{self, Write as _},
    path::PathBuf,
    thread,
    time::Duration,
};

use crossterm::terminal;
use jp_inquire::{InlineOption, InlineReply, InlineSelect, release_terminal, terminal_held};

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
