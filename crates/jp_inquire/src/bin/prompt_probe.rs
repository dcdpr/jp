//! Test probe for PTY tests that opens a prompt and prints its outcome.
//!
//! The first argument selects the prompt:
//!
//! - `select`: [`InlineSelect`] with two options.
//! - `select-default`: [`InlineSelect`] with a default option and help message.
//! - `reply`: [`InlineReply`].
//!
//! If a second argument is provided, a background thread polls for that file to
//! appear and then calls [`close_all`].
//! Only `reply` responds to `close_all`; `inquire`-backed selects wait for a
//! keystroke.

use std::{
    env,
    io::{self, Write as _},
    path::PathBuf,
    thread,
    time::Duration,
};

use crossterm::terminal;
use jp_inquire::{InlineOption, InlineReply, InlineSelect, close_all};

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
