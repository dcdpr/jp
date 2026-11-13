//! A program a pty test spawns to open one prompt and report how it ended.
//!
//! The first argument names the prompt:
//!
//! - `select`: an [`InlineSelect`] with two options.
//! - `select-default`: the same, with a default and a help message.
//! - `reply`: an [`InlineReply`].
//!
//! With a second argument, the probe closes every prompt once the file it names
//! appears, the way a program that is shutting down does.
//! Only the reply acts on that: `inquire` draws the select, and waits for a
//! key.
//!
//! For a reply, the terminal is put in raw mode before anything is printed, so
//! the test can answer reedline's cursor-position query as soon as the first
//! row appears: typed earlier, the answer would be echoed onto the screen.
//!
//! When the prompt returns, the probe prints its outcome on the row the cursor
//! was left on.

use std::{
    env,
    io::{self, Write as _},
    path::PathBuf,
    thread,
    time::Duration,
};

use crossterm::terminal;
use jp_inquire::{InlineOption, InlineReply, InlineSelect, close_all};

/// How often the probe looks for the file that tells it to close its prompt.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn main() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let prompt = args.next().ok_or_else(|| io::Error::other("no prompt"))?;

    if let Some(close) = args.next().map(PathBuf::from) {
        thread::spawn(move || {
            while !close.exists() {
                thread::sleep(POLL_INTERVAL);
            }
            close_all();
        });
    }

    let mut stdout = io::stdout();

    if prompt == "reply" {
        terminal::enable_raw_mode()?;
        write!(stdout, "above\r\n")?;
        stdout.flush()?;

        let outcome = InlineReply::new("Reply:").prompt(Box::new(io::stdout()));

        terminal::disable_raw_mode()?;
        writeln!(stdout, "outcome: {outcome:?}")?;
        return stdout.flush();
    }

    writeln!(stdout, "above")?;

    let options = vec![
        InlineOption::new('y', "remove it"),
        InlineOption::new('n', "keep it"),
    ];
    let mut select = InlineSelect::new("Remove it?", options);
    if prompt == "select-default" {
        select = select
            .with_default('n')
            .with_help_message("cannot be undone");
    }

    let outcome = select.prompt(&mut stdout);
    writeln!(stdout, "outcome: {outcome:?}")?;
    stdout.flush()
}
