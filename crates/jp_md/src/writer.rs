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
    format::{BackgroundFill, DefaultBackground},
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
    pub(crate) attrs: AnsiState,

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
        let default_background = default_background.copied();

        // Pre-populate attrs so the restore logic keeps the default background
        // active across line breaks.
        let attrs = default_background.map_or_else(AnsiState::default, |bg| AnsiState {
            background: Some(format!("48;5;{}", bg.color)),
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
    fn flush_wrap_buffer(&mut self) -> fmt::Result {
        let len = self.wrap_buffer.len();
        let merged = self.merge_escapes(0, len);
        self.output.write_str(&merged)?;
        self.wrap_buffer.clear();
        self.escapes.clear();
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
        self.window
            .extend_from_slice(&self.prefix.as_bytes()[self.prefix.len() - 2..]);
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
        let Some(bg) = self.default_background else {
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
                self.write_escape("\x1b[K")?;
            }
        }
        Ok(())
    }

    /// Like [`emit_line_fill`](Self::emit_line_fill) but writes directly to the
    /// output writer. Used in the wrap-break path.
    fn emit_line_fill_direct(&mut self) -> fmt::Result {
        let Some(bg) = self.default_background else {
            return Ok(());
        };

        match bg.fill {
            BackgroundFill::Content => {}
            BackgroundFill::Column(target) => {
                let pad = target.saturating_sub(self.column);
                if pad > 0 {
                    for _ in 0..pad {
                        self.output.write_char(' ')?;
                    }
                    self.column += pad;
                }
            }
            BackgroundFill::Terminal => {
                self.output.write_str("\x1b[K")?;
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
                    self.write_prefix()?;
                    if self.pending_attr_restore {
                        self.restore_attrs_after_prefix()?;
                        self.pending_attr_restore = false;
                    }
                    self.column = self.prefix_width();
                }
            }
            self.need_cr -= 1;
        }

        let mut it = s.char_indices();

        while let Some((i, c)) = it.next() {
            if self.begin_line {
                self.write_prefix()?;
                self.column = self.prefix_width();
                if self.pending_attr_restore {
                    self.restore_attrs_after_prefix()?;
                    self.pending_attr_restore = false;
                }
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
                let break_pos = self.last_breakable;

                let first_part = self.merge_escapes(0, break_pos);
                self.output.write_str(&first_part)?;
                self.emit_line_fill_direct()?;
                if self.attrs.is_active() {
                    self.output.write_str(RESET)?;
                }
                self.output.write_str("\n")?;

                let rest_start = break_pos + 1;
                let rest = if rest_start < self.wrap_buffer.len() {
                    self.wrap_buffer[rest_start..].to_string()
                } else {
                    String::new()
                };

                self.wrap_buffer.clear();
                self.escapes.clear();

                self.wrap_buffer.push_str(&self.prefix);
                if self.attrs.is_active() {
                    let prefix_len = self.prefix.len();
                    self.escapes
                        .push((prefix_len, self.attrs.restore_sequence()));
                }
                self.wrap_buffer.push_str(&rest);
                self.column = self.prefix_width() + ansi::visual_width(&rest);
                self.last_breakable = 0;
                self.begin_line = false;
                self.begin_content = false;
                self.pending_attr_restore = false;
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
    pub(crate) fn write_raw(&mut self, s: &str) -> fmt::Result {
        if !self.wrap_buffer.is_empty() {
            self.flush_wrap_buffer()?;
        }
        while self.need_cr > 0 {
            self.output.write_str("\n")?;
            self.need_cr -= 1;
        }
        self.output.write_str(s)?;
        self.column = 0;
        self.begin_line = true;
        self.begin_content = true;
        self.window.clear();
        self.window.push(b'\n');

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
