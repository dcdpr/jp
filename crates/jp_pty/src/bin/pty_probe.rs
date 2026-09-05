//! A program for a pty test to spawn.
//!
//! Reports the terminal size it was given, then echoes each line it reads and
//! reports the size again, so a test can watch a resize reach a running child.
//! Exits on `quit` or when its input closes.
//!
//! It exists because the spawn path is the only way into a pty on Windows, and
//! a test of that path needs a child that behaves the same on every platform.

use std::io::{self, BufRead as _, Write};

/// Read lines until one says `quit`, reporting the terminal size around each.
fn main() {
    let mut out = io::stdout();
    report(&mut out);

    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim_end();
        if line == "quit" {
            break;
        }

        let _err = writeln!(out, "you typed: {line}");
        report(&mut out);
    }
}

/// Write the terminal's size, or `0x0` when it cannot be measured.
fn report(out: &mut impl Write) {
    let (columns, rows) = crossterm::terminal::size().unwrap_or((0, 0));

    let _err = writeln!(out, "size {rows}x{columns}");
    let _err = out.flush();
}
