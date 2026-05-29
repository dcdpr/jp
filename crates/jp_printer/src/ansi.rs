//! Streaming ANSI escape-sequence stripping for non-pretty output.
//!
//! [`AnsiStripper`] wraps a writer and removes ANSI escape sequences from the
//! bytes written through it.
//! It holds a [`vte::Parser`] across `write` calls, so a sequence split over
//! several writes (as `write!`/`writeln!` and crossterm both do, one piece per
//! `write_str`) is still recognized and removed.
//! A non-persistent strip sees only fragments and would drop the escape
//! introducers while leaving the parameter bytes behind.

use std::io::{self, Write};

use vte::{Parser, Perform};

/// A writer that strips ANSI escape sequences as bytes flow through it.
///
/// Parser state persists across `write` calls.
/// Printable bytes are forwarded to the inner writer as soon as they are
/// parsed; only the bytes of an in-progress escape sequence are withheld, and
/// those are dropped once the sequence completes.
/// Surrounding text is never buffered to a line boundary.
pub struct AnsiStripper<W> {
    /// The VTE state machine driving [`StripSink`].
    parser: Parser,

    /// Forwards printable output and absorbs escape sequences.
    sink: StripSink<W>,
}

impl<W: Write> AnsiStripper<W> {
    /// Wrap `inner`, stripping ANSI escapes from everything written to it.
    pub fn new(inner: W) -> Self {
        Self {
            parser: Parser::new(),
            sink: StripSink { inner, err: None },
        }
    }
}

impl<W: Write> Write for AnsiStripper<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.parser.advance(&mut self.sink, buf);
        self.sink.err.take().map_or(Ok(buf.len()), Err)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink.inner.flush()
    }
}

/// [`Perform`] target that writes printable output straight to the inner writer
/// and discards escape sequences.
struct StripSink<W> {
    /// The destination for stripped output.
    inner: W,

    /// First I/O error seen while performing, surfaced by the next `write`.
    err: Option<io::Error>,
}

impl<W: Write> Perform for StripSink<W> {
    fn print(&mut self, c: char) {
        if self.err.is_some() {
            return;
        }
        let mut buf = [0_u8; 4];
        self.err = self
            .inner
            .write_all(c.encode_utf8(&mut buf).as_bytes())
            .err();
    }

    fn execute(&mut self, byte: u8) {
        // Keep line feeds, drop every other C0 control (carriage returns, tabs,
        // …) along with the escape sequences themselves. This matches what
        // `strip_ansi_escapes::strip_str` did before stripping moved here.
        if byte == b'\n' && self.err.is_none() {
            self.err = self.inner.write_all(&[byte]).err();
        }
    }
}

#[cfg(test)]
#[path = "ansi_tests.rs"]
mod tests;
