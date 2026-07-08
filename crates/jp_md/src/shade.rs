//! A streaming writer that maintains a default-background fill across a byte
//! stream.
//!
//! [`ShadedWriter`] wraps another [`Write`] and keeps a region background `B`
//! showing from column 0 to the right edge of every visual line — including
//! lines produced by carriage-return rewrites (`\r`) and `\x1b[K` erases —
//! while stepping out of the way for any background the content sets itself.
//! [`shade`] is the buffer-at-a-time convenience built on the same core.
//!
//! Unlike the line-oriented [`apply_line_background`], the writer holds its
//! state across `write_str` calls, so it can keep `B` active across cursor
//! rewrites and escape sequences split between writes — the cases a pure
//! per-line transform cannot express.
//!
//! [`apply_line_background`]: crate::format::apply_line_background

use std::fmt::{self, Write};

use crate::{
    ansi::{self, AnsiState, Segment},
    format::{BackgroundFill, DefaultBackground},
};

/// Wraps a writer and maintains a default-background invariant across the byte
/// stream, preserving any background the content sets itself.
///
/// As bytes flow through, the region background `B` is asserted before content
/// that would otherwise render on the terminal default, each completed line is
/// filled to the right edge with the active background, and `B` is re-asserted
/// after a content reset.
/// Spans where the content sets its own background are left untouched.
/// [`finish`] ends the region, clearing `B` so it does not leak.
///
/// The tracked content [`AnsiState`] and the partial-escape buffer persist
/// across `write_str` calls, so a sequence or line boundary split between
/// writes is still handled correctly.
///
/// [`finish`]: Self::finish
pub struct ShadedWriter<W: Write> {
    /// The wrapped writer that receives the shaded byte stream.
    output: W,

    /// The region background escape (`\x1b[{param}m`), pre-rendered.
    background_escape: String,

    /// Whether the region fills each line to the right edge with `\x1b[K`.
    ///
    /// True for [`BackgroundFill::Terminal`] (the reasoning background);
    /// otherwise the background only backs the content itself.
    fill_to_edge: bool,

    /// The content's currently active attributes, fed only the content's own
    /// escapes — never the writer's injected background.
    content: AnsiState,

    /// Whether the region background must be (re-)asserted before the next
    /// visible content or content erase.
    ///
    /// Set at construction and after a content reset/background-clear; cleared
    /// once `B` is asserted or once the content sets its own background.
    /// A line boundary does not set it: the terminal keeps `B` across
    /// `\n`/`\r`, so no re-assert is owed there.
    needs_background: bool,

    /// Holds a trailing escape sequence split across a write boundary,
    /// completed and processed on the next write.
    pending: String,
}

impl<W: Write> ShadedWriter<W> {
    /// Wrap `output`, shading everything written through it with `background`.
    #[must_use]
    pub fn new(output: W, background: &DefaultBackground) -> Self {
        Self {
            output,
            background_escape: format!("\x1b[{}m", background.param),
            fill_to_edge: matches!(background.fill, BackgroundFill::Terminal),
            content: AnsiState::default(),
            needs_background: true,
            pending: String::new(),
        }
    }

    /// End the shaded region.
    ///
    /// Flushes any escape sequence still buffered from a split write, then
    /// emits `\x1b[49m` so the region background does not leak past the region.
    /// Call once after the last write.
    ///
    /// # Errors
    ///
    /// Propagates any error from the wrapped writer.
    pub fn finish(&mut self) -> fmt::Result {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.output.write_str(&pending)?;
        }

        // Clear the background only when one is actually active: either we
        // asserted `B` (and the content hasn't reset it) or the content left
        // its own background open. An empty or already-reset region needs
        // nothing.
        if !self.needs_background || self.content.background.is_some() {
            self.output.write_str(ansi::BG_END)?;
        }

        Ok(())
    }

    /// Forward one complete escape, applying the invariant to SGR and CSI-erase
    /// sequences and passing everything else through verbatim.
    fn process_escape(&mut self, esc: &str) -> fmt::Result {
        if is_sgr(esc) {
            let affects_background = self.content.update(esc);
            self.output.write_str(esc)?;
            if affects_background {
                // The escape changed the background landscape: the content now
                // owns the background (suppress `B`), or it was cleared/reset
                // (re-assert `B` before the next content).
                self.needs_background = self.content.background.is_none();
            }
            return Ok(());
        }

        if is_erase(esc) {
            // The erase fills with the content's background when it has one, and
            // with the region background otherwise.
            self.ensure_background()?;
            return self.output.write_str(esc);
        }

        // Cursor moves and other escapes don't affect the background invariant.
        self.output.write_str(esc)
    }

    /// Forward visible text, asserting the region background under it and
    /// filling each completed line to the right edge.
    fn process_text(&mut self, text: &str) -> fmt::Result {
        let mut start = 0;
        for (i, byte) in text.bytes().enumerate() {
            match byte {
                b'\n' => {
                    self.emit_run(&text[start..i])?;
                    self.fill_line()?;
                    self.output.write_str("\n")?;
                    start = i + 1;
                }
                b'\r' => {
                    self.emit_run(&text[start..i])?;
                    self.output.write_str("\r")?;
                    start = i + 1;
                }
                _ => {}
            }
        }
        self.emit_run(&text[start..])
    }

    /// Emit a run of visible text with the region background asserted under it.
    fn emit_run(&mut self, run: &str) -> fmt::Result {
        if run.is_empty() {
            return Ok(());
        }
        self.ensure_background()?;
        self.output.write_str(run)
    }

    /// Fill the current line to the right edge with the active background,
    /// ahead of a newline.
    fn fill_line(&mut self) -> fmt::Result {
        self.ensure_background()?;
        if self.fill_to_edge {
            self.output.write_str("\x1b[K")?;
        }
        Ok(())
    }

    /// Assert the region background if it is owed and the content hasn't set
    /// its own.
    fn ensure_background(&mut self) -> fmt::Result {
        if self.needs_background && self.content.background.is_none() {
            self.output.write_str(&self.background_escape)?;
            self.needs_background = false;
        }
        Ok(())
    }
}

impl<W: Write> Write for ShadedWriter<W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Re-attach any escape sequence held back from the previous write, so a
        // sequence split across writes is processed as a whole.
        let mut combined = std::mem::take(&mut self.pending);
        combined.push_str(s);

        let segments: Vec<Segment<'_>> = ansi::segments(&combined).collect();
        let last = segments.len().saturating_sub(1);
        for (i, segment) in segments.into_iter().enumerate() {
            match segment {
                // A trailing escape cut off at the write boundary is buffered
                // and completed on the next write, not forwarded half-formed.
                Segment::Escape(esc) if i == last && is_incomplete(esc) => {
                    self.pending.push_str(esc);
                }
                Segment::Escape(esc) => self.process_escape(esc)?,
                Segment::Text(text) => self.process_text(text)?,
            }
        }

        Ok(())
    }
}

/// Shade `text` with `background`, returning the result as a new string.
///
/// A convenience wrapper that runs a [`ShadedWriter`] over an owned buffer.
/// Used by replay and tests; streaming callers write through [`ShadedWriter`]
/// directly.
#[must_use]
pub fn shade(text: &str, background: &DefaultBackground) -> String {
    let mut buffer = String::new();
    {
        let mut writer = ShadedWriter::new(&mut buffer, background);
        // Writing to a `String` is infallible.
        let _ = writer.write_str(text);
        let _ = writer.finish();
    }
    buffer
}

/// Whether `esc` is an SGR sequence (`\x1b[…m`).
fn is_sgr(esc: &str) -> bool {
    esc.starts_with("\x1b[") && esc.ends_with('m')
}

/// Whether `esc` is a CSI erase-in-line (`\x1b[K`, `\x1b[2K`, …).
fn is_erase(esc: &str) -> bool {
    esc.starts_with("\x1b[") && esc.ends_with('K')
}

/// Whether `esc` is a partial escape that never reached its terminating byte,
/// i.e. it was cut off at a write boundary.
///
/// CSI and simple escapes terminate with an ASCII letter or `~`; OSC string
/// sequences (`\x1b]…`) terminate with BEL or ST (`\x1b\\`).
fn is_incomplete(esc: &str) -> bool {
    if esc.starts_with("\x1b]") {
        return !(esc.ends_with('\x07') || esc.ends_with("\x1b\\"));
    }
    !esc.chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '~')
}

#[cfg(test)]
#[path = "shade_tests.rs"]
mod tests;
