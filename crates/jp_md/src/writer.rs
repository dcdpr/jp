//! ANSI-aware terminal writer with word-wrapping support.
//!
//! [`TerminalWriter`] handles the low-level concerns of writing styled text to
//! a terminal: word-wrapping at a target width, tracking ANSI escape sequences
//! separately from visible text, and managing background fills across line
//! breaks.
//!
//! The AST renderer in [`render`](crate::render) drives this writer by calling
//! [`output`](TerminalWriter::output) for visible text and
//! [`write_escape`](TerminalWriter::write_escape) for ANSI codes.

use std::{
    cmp::max,
    fmt::{self, Write},
};

use crate::{
    ansi::{self, AnsiState, RESET},
    format::{self, BackgroundFill, DefaultBackground},
};

/// ANSI-aware terminal writer with word-wrapping support.
#[expect(clippy::struct_excessive_bools)]
pub struct TerminalWriter<'w> {
    /// The output writer.
    output: &'w mut dyn Write,

    /// Buffer containing only visible text (no ANSI escapes).
    /// Flushed on newline or wrap.
    wrap_buffer: String,

    /// ANSI escape sequences keyed by byte offset in `wrap_buffer`.
    /// Each `(offset, code)` means "emit `code` just before the byte
    /// at `offset`". Offsets equal to `wrap_buffer.len()` are valid.
    escapes: Vec<(usize, String)>,

    /// The last two bytes written (for newline detection).
    window: Vec<u8>,

    /// Byte offset in `wrap_buffer` of the last breakable space.
    last_breakable: usize,

    /// Accumulated prefix for blockquotes and list indentation.
    pub(crate) prefix: String,

    /// Current visual column (ANSI escapes do NOT count).
    pub(crate) column: usize,

    /// Pending newlines to emit.
    pub(crate) need_cr: u8,

    /// Whether we are at the beginning of a line.
    pub(crate) begin_line: bool,

    /// Whether we are at the beginning of content (for list marker checks).
    pub(crate) begin_content: bool,

    /// Whether line breaks are suppressed (e.g. inside headings).
    pub(crate) no_linebreaks: bool,

    /// Whether we are in a tight list item.
    pub(crate) in_tight_list_item: bool,

    /// Whether attrs need restoring after the next prefix write.
    pending_attr_restore: bool,

    /// Whether wrapping is enabled.
    allow_wrap: bool,

    /// Target line width (0 = no wrapping).
    pub(crate) width: usize,

    /// Currently active ANSI attributes.
    ///
    /// This is the renderer's running view of the *intended* state — i.e. the
    /// state implied by every escape ever pushed since the last full reset.
    /// It is not necessarily the state the terminal is in right now: some of
    /// those escapes may still be sitting in [`escapes`](Self::escapes),
    /// waiting to be flushed.
    pub(crate) attrs: AnsiState,

    /// ANSI state at the start of the current `wrap_buffer` batch.
    ///
    /// Replaying every escape in [`escapes`](Self::escapes) on top of this
    /// state yields [`attrs`](Self::attrs). Used during wrap-break to
    /// reconstruct the state at an arbitrary byte offset (specifically, at the
    /// break point) without losing escapes that were recorded past it.
    batch_initial_attrs: AnsiState,

    /// Optional default background color for all content.
    pub(crate) default_background: Option<DefaultBackground>,
}

impl<'w> TerminalWriter<'w> {
    /// Create a new terminal writer.
    pub(crate) fn new(
        output: &'w mut dyn Write,
        width: usize,
        default_background: Option<&DefaultBackground>,
    ) -> Self {
        let default_background = default_background.cloned();

        // Pre-populate attrs so the restore logic keeps the default background
        // active across line breaks.
        let attrs = default_background
            .as_ref()
            .map_or_else(AnsiState::default, |bg| AnsiState {
                background: Some(bg.param.clone()),
                ..Default::default()
            });

        Self {
            output,
            wrap_buffer: String::new(),
            escapes: Vec::new(),
            window: Vec::with_capacity(2),
            prefix: String::new(),
            column: 0,
            need_cr: 0,
            last_breakable: 0,
            begin_line: true,
            begin_content: true,
            no_linebreaks: false,
            in_tight_list_item: false,
            pending_attr_restore: false,
            allow_wrap: width > 0,
            width,
            attrs,
            // The terminal starts in default state. The first batch's escapes
            // (including any default-background setup) are responsible for
            // bringing it to `attrs`.
            batch_initial_attrs: AnsiState::default(),
            default_background,
        }
    }

    /// Visual width of the prefix in terminal columns.
    pub(crate) fn prefix_width(&self) -> usize {
        ansi::visual_width(&self.prefix)
    }

    /// Record an ANSI escape at the current position in the wrap buffer.
    ///
    /// The escape is stored in a side-channel and injected into the
    /// output at the correct byte offset when the buffer is flushed or
    /// split by the wrapping logic.
    pub(crate) fn write_escape(&mut self, code: &str) -> fmt::Result {
        if self.width == 0 {
            self.output.write_str(code)
        } else {
            let offset = self.wrap_buffer.len();
            self.escapes.push((offset, code.to_string()));
            Ok(())
        }
    }

    /// Merge visible text from `wrap_buffer[start..end]` with ANSI escapes
    /// that fall within that byte range.
    fn merge_escapes(&self, start: usize, end: usize) -> String {
        let text = &self.wrap_buffer[start..end];
        let mut result = String::with_capacity(text.len() * 2);
        let mut text_pos = start;
        for (offset, code) in &self.escapes {
            if *offset < start {
                continue;
            }
            if *offset > end {
                break;
            }
            let local = *offset - start;
            let chunk_start = text_pos - start;
            if local > chunk_start {
                result.push_str(&text[chunk_start..local]);
            }
            result.push_str(code);
            text_pos = *offset;
        }
        let chunk_start = text_pos - start;
        if chunk_start < text.len() {
            result.push_str(&text[chunk_start..]);
        }
        result
    }

    /// Flush the entire wrap buffer (with escapes) to the output writer.
    ///
    /// Callers are expected to push a trailing `RESET` escape before flushing
    /// when `attrs` is active, so the terminal state is `default` afterwards.
    /// `batch_initial_attrs` is reset accordingly.
    fn flush_wrap_buffer(&mut self) -> fmt::Result {
        let len = self.wrap_buffer.len();
        let merged = self.merge_escapes(0, len);
        self.output.write_str(&merged)?;
        self.wrap_buffer.clear();
        self.escapes.clear();
        self.batch_initial_attrs = AnsiState::default();
        Ok(())
    }

    /// Soft-wrap the current wrap buffer at `last_breakable`.
    ///
    /// Splits `wrap_buffer` at the last breakable space and starts a fresh
    /// line with the prefix and the remainder. Escapes are partitioned by
    /// their offset:
    ///
    /// - Escapes at offsets `≤ break_pos` are folded into the *first part*
    ///   (already done by [`merge_escapes`](Self::merge_escapes)) and replayed
    ///   onto [`batch_initial_attrs`](Self::batch_initial_attrs) to derive
    ///   the attribute state at the break point.
    /// - Escapes at offsets `> break_pos` belong to the *rest* and are
    ///   re-anchored onto the new buffer at
    ///   `prefix.len() + (offset - rest_start)`. Dropping them — as a
    ///   previous version of this code did — caused mid-line attribute
    ///   changes (e.g. opening `**`) to bleed back to the start of the
    ///   continuation line.
    fn wrap_break(&mut self) -> fmt::Result {
        let break_pos = self.last_breakable;
        let rest_start = break_pos + 1;

        // Emit everything up to (and including escapes anchored at) break_pos.
        let first_part = self.merge_escapes(0, break_pos);
        self.output.write_str(&first_part)?;

        // Partition the escapes:
        //   - `≤ break_pos`: replay onto attrs_at_break.
        //   - `> break_pos`:  carry forward onto the new line.
        let mut attrs_at_break = self.batch_initial_attrs.clone();
        let mut late_escapes: Vec<(usize, String)> = Vec::new();
        for (offset, code) in self.escapes.drain(..) {
            if offset <= break_pos {
                attrs_at_break.update(&code);
            } else {
                let new_offset = self.prefix.len() + (offset - rest_start);
                late_escapes.push((new_offset, code));
            }
        }

        self.emit_line_fill_direct()?;
        if attrs_at_break.is_active() {
            self.output.write_str(RESET)?;
        }
        self.output.write_str("\n")?;

        let rest = if rest_start < self.wrap_buffer.len() {
            self.wrap_buffer[rest_start..].to_string()
        } else {
            String::new()
        };
        self.wrap_buffer.clear();

        // Re-establish the styling that was active at break_pos for the new
        // line. Late escapes take it from there.
        if attrs_at_break.is_active() {
            let has_temp_bg = self.default_background.is_some()
                && attrs_at_break.background
                    != self.default_background.as_ref().map(|bg| bg.param.clone());

            if has_temp_bg {
                // The prefix gets the default (reasoning) bg so a
                // temporary bg (e.g. inline code) doesn't bleed into the
                // indentation.
                let mut prefix_attrs = attrs_at_break.clone();
                prefix_attrs.background =
                    self.default_background.as_ref().map(|bg| bg.param.clone());
                self.escapes.push((0, prefix_attrs.restore_sequence()));
                // After the prefix, restore the actual attrs (including the
                // temporary bg) for the content.
                self.escapes
                    .push((self.prefix.len(), attrs_at_break.restore_sequence()));
            } else {
                self.escapes.push((0, attrs_at_break.restore_sequence()));
            }
        }

        self.wrap_buffer.push_str(&self.prefix);
        self.wrap_buffer.push_str(&rest);
        self.escapes.extend(late_escapes);

        // The terminal is in default state now (either RESET was emitted, or
        // attrs_at_break was already inactive). The new batch's escapes
        // bring it back up to `attrs`.
        self.batch_initial_attrs = AnsiState::default();

        self.column = self.prefix_width() + ansi::visual_width(&rest);
        self.last_breakable = 0;
        self.begin_line = false;
        self.begin_content = false;
        self.pending_attr_restore = false;

        Ok(())
    }

    /// Write visible content, updating window and column.
    fn write_visible(&mut self, s: &str) -> fmt::Result {
        if s.is_empty() {
            return Ok(());
        }

        // Maintain a window of the last 2 bytes.
        if s.len() > 1 {
            self.window.clear();
            self.window.extend_from_slice(&s.as_bytes()[s.len() - 2..]);
        } else {
            if self.window.len() == 2 {
                self.window.remove(0);
            }
            self.window.push(s.as_bytes()[0]);
        }

        let last_was_cr = self.window.last() == Some(&b'\n');

        if self.width == 0 {
            if last_was_cr && self.default_background.is_some() {
                self.emit_line_fill_direct()?;
                if self.attrs.is_active() {
                    self.output.write_str(RESET)?;
                }
                self.output.write_str(s)?;
                if self.attrs.is_active() {
                    self.output.write_str(&self.attrs.restore_sequence())?;
                }
            } else {
                self.output.write_str(s)?;
            }
        } else if last_was_cr {
            self.emit_line_fill()?;
            if self.attrs.is_active() {
                let offset = self.wrap_buffer.len();
                self.escapes.push((offset, RESET.to_string()));
            }
            self.flush_wrap_buffer()?;
            self.output.write_str(s)?;
        } else {
            self.wrap_buffer.push_str(s);
        }

        if last_was_cr {
            self.column = 0;
            self.begin_line = true;
            self.begin_content = true;
            self.last_breakable = 0;
            if self.attrs.is_active() {
                self.pending_attr_restore = true;
            }
        }

        Ok(())
    }

    /// Write the accumulated prefix (blockquote `> `, list indent, etc.).
    fn write_prefix(&mut self) -> fmt::Result {
        if self.prefix.is_empty() {
            return Ok(());
        }
        if self.width == 0 {
            self.output.write_str(&self.prefix)?;
        } else {
            self.wrap_buffer.push_str(&self.prefix);
        }
        self.window.clear();
        let prefix_bytes = self.prefix.as_bytes();
        let start = prefix_bytes.len().saturating_sub(2);
        self.window.extend_from_slice(&prefix_bytes[start..]);
        Ok(())
    }

    /// Restore active attributes after a line break + prefix.
    fn restore_attrs_after_prefix(&mut self) -> fmt::Result {
        if self.attrs.is_active() {
            let seq = self.attrs.restore_sequence();
            self.write_escape(&seq)?;
        }
        Ok(())
    }

    /// Emit the background fill for the current line.
    ///
    /// Depending on the [`BackgroundFill`] mode this either:
    /// - does nothing (`Content`),
    /// - pads with spaces to a fixed column (`Column`),
    /// - emits `\x1b[K` to fill to the terminal edge (`Terminal`).
    fn emit_line_fill(&mut self) -> fmt::Result {
        let Some(ref bg) = self.default_background else {
            return Ok(());
        };

        match bg.fill {
            BackgroundFill::Content => {}
            BackgroundFill::Column(target) => {
                let pad = target.saturating_sub(self.column);
                if pad > 0 {
                    let spaces: String = " ".repeat(pad);
                    if self.width == 0 {
                        self.output.write_str(&spaces)?;
                    } else {
                        self.wrap_buffer.push_str(&spaces);
                    }
                    self.column += pad;
                }
            }
            BackgroundFill::Terminal => {
                // Ensure the default background is active before erase,
                // so a temporary background (e.g. inline code) doesn't
                // bleed to the terminal edge.
                self.write_escape(&format!("\x1b[{}m\x1b[K", bg.param))?;
            }
        }
        Ok(())
    }

    /// Like [`emit_line_fill`](Self::emit_line_fill) but writes directly to the
    /// output writer. Used in the wrap-break path.
    fn emit_line_fill_direct(&mut self) -> fmt::Result {
        let Some(ref bg) = self.default_background else {
            return Ok(());
        };

        match bg.fill {
            BackgroundFill::Content => {}
            BackgroundFill::Column(target) => {
                let pad = target.saturating_sub(self.column);
                if pad > 0 {
                    // Set default bg before padding so inline code
                    // background doesn't bleed into the padding.
                    self.output.write_str(&format!("\x1b[{}m", bg.param))?;
                    for _ in 0..pad {
                        self.output.write_char(' ')?;
                    }
                    self.column += pad;
                }
            }
            BackgroundFill::Terminal => {
                // Set default bg before erase so inline code background
                // doesn't bleed to the terminal edge.
                self.output
                    .write_str(&format!("\x1b[{}m\x1b[K", bg.param))?;
            }
        }
        Ok(())
    }

    /// Output visible text with optional wrapping support.
    pub(crate) fn output(&mut self, s: &str, wrap: bool) -> fmt::Result {
        let bytes = s.as_bytes();
        let wrap = self.allow_wrap && wrap && !self.no_linebreaks;

        if self.in_tight_list_item && self.need_cr > 1 {
            self.need_cr = 1;
        }

        // Emit pending newlines.
        let mut last_crs_consume = self
            .window
            .iter()
            .rev()
            .take_while(|&&b| b == b'\n')
            .count();
        while self.need_cr > 0 {
            if self.window.is_empty() {
                // nop
            } else if last_crs_consume > 0 {
                last_crs_consume -= 1;
            } else {
                self.write_visible("\n")?;
                if self.need_cr > 1 {
                    if self.pending_attr_restore {
                        self.restore_attrs_after_prefix()?;
                        self.pending_attr_restore = false;
                    }
                    self.write_prefix()?;
                    self.column = self.prefix_width();
                }
            }
            self.need_cr -= 1;
        }

        let mut it = s.char_indices();

        while let Some((i, c)) = it.next() {
            if self.begin_line {
                if self.pending_attr_restore {
                    // Restore ANSI attributes before the prefix so visible
                    // prefix characters (e.g. blockquote '>') get styled.
                    self.restore_attrs_after_prefix()?;
                    self.pending_attr_restore = false;
                }
                self.write_prefix()?;
                self.column = self.prefix_width();
            }

            let nextb = bytes.get(i + 1);
            if c == ' ' && wrap {
                if !self.begin_line {
                    let last_nonspace = self.wrap_buffer.len();
                    self.write_visible(" ")?;
                    self.column += 1;
                    self.begin_line = false;
                    self.begin_content = false;
                    // Skip extra spaces.
                    while let Some((_, ' ')) = it.clone().next() {
                        it.next();
                    }
                    if !nextb.is_some_and(|&c| c.is_ascii_digit()) {
                        self.last_breakable = last_nonspace;
                    }
                }
            } else if bytes[i] == b'\n' {
                self.write_visible("\n")?;
            } else {
                let cs = c.to_string();
                self.write_visible(&cs)?;
                // Per-char width: O(1). May be off by 1 for VS16/ZWJ
                // sequences, but using visual_width(&wrap_buffer) here
                // would be O(n) per char → O(n²) per line.
                self.column += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                self.begin_line = false;
                self.begin_content = self.begin_content && bytes[i].is_ascii_digit();
            }

            // Check if we need to wrap.
            if self.width > 0
                && self.column > self.width
                && !self.begin_line
                && self.last_breakable > 0
            {
                self.wrap_break()?;
            }
        }

        Ok(())
    }

    /// Request at least one newline before the next content.
    pub(crate) fn cr(&mut self) {
        self.need_cr = max(self.need_cr, 1);
    }

    /// Request a blank line before the next content.
    pub(crate) fn blankline(&mut self) {
        self.need_cr = max(self.need_cr, 2);
    }

    /// Write pre-formatted content directly to output, bypassing wrapping.
    ///
    /// Flushes any pending wrap buffer content first, emits pending newlines,
    /// then writes `s` directly. Resets line tracking state afterward (column,
    /// `begin_line`, window).
    ///
    /// When a default background is active, the background escape is injected
    /// at the start of each line and line-fill is applied before each newline,
    /// so syntax-highlighted code blocks inherit the reasoning background.
    pub(crate) fn write_raw(&mut self, s: &str) -> fmt::Result {
        if !self.wrap_buffer.is_empty() {
            self.flush_wrap_buffer()?;
        }

        // Emit pending newlines with line fill when background is active.
        while self.need_cr > 0 {
            if self.default_background.is_some() {
                self.emit_line_fill_direct()?;
                if self.attrs.is_active() {
                    self.output.write_str(RESET)?;
                }
            }
            self.output.write_str("\n")?;
            self.need_cr -= 1;
        }

        if let Some(ref bg) = self.default_background {
            let with_bg = format::apply_line_background(s, Some(bg));
            self.output.write_str(&with_bg)?;
        } else {
            self.output.write_str(s)?;
        }

        self.column = 0;
        self.begin_line = true;
        self.begin_content = true;
        self.window.clear();
        self.window.push(b'\n');

        // The raw content likely ends with RESET, so subsequent output
        // calls need to re-establish active attrs.
        if self.attrs.is_active() {
            self.pending_attr_restore = true;
        }

        Ok(())
    }

    /// Flush remaining content and finalize output.
    ///
    /// Emits line fill, resets ANSI attributes, and ensures output ends with a
    /// newline.
    pub(crate) fn finish(&mut self) -> fmt::Result {
        if !self.wrap_buffer.is_empty() {
            self.emit_line_fill()?;
            if self.attrs.is_active() {
                let offset = self.wrap_buffer.len();
                self.escapes.push((offset, RESET.to_string()));
            }
            self.flush_wrap_buffer()?;
        } else if self.default_background.is_some() {
            self.emit_line_fill_direct()?;
            if self.attrs.is_active() {
                self.output.write_str(RESET)?;
            }
        }

        if !self.window.is_empty() && self.window.last() != Some(&b'\n') {
            self.output.write_str("\n")?;
        }

        Ok(())
    }
}

impl Write for TerminalWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.output(s, false)
    }
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
