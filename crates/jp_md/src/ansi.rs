//! Shared ANSI SGR escape constants and state tracking.
//!
//! This module provides the escape sequences, state tracking, and visual width
//! computation used by both the terminal renderer (`render.rs`) and the table
//! formatter (`table.rs`).

/// SGR: Bold on.
pub const BOLD_START: &str = "\x1b[1m";

/// SGR: Bold off.
pub const BOLD_END: &str = "\x1b[22m";

/// SGR: Italic on.
pub const ITALIC_START: &str = "\x1b[3m";

/// SGR: Italic off.
pub const ITALIC_END: &str = "\x1b[23m";

/// SGR: Underline on.
pub const UNDERLINE_START: &str = "\x1b[4m";

/// SGR: Underline off.
pub const UNDERLINE_END: &str = "\x1b[24m";

/// SGR: Strikethrough on.
pub const STRIKETHROUGH_START: &str = "\x1b[9m";

/// SGR: Strikethrough off.
pub const STRIKETHROUGH_END: &str = "\x1b[29m";

/// SGR: Background color reset.
pub const BG_END: &str = "\x1b[49m";

/// SGR: Foreground color reset.
pub const FG_END: &str = "\x1b[39m";

/// SGR: Full attribute reset.
pub const RESET: &str = "\x1b[0m";

/// Tracks which ANSI SGR attributes are currently active.
///
/// Used to close formatting at line breaks and re-open it on the next line,
/// both for the terminal renderer's incremental wrapping and the table
/// formatter's batch wrapping.
#[derive(Debug, Clone, Default)]
#[expect(clippy::struct_excessive_bools)]
pub struct AnsiState {
    /// Bold text (SGR 1 / 22).
    pub bold: bool,

    /// Italic text (SGR 3 / 23).
    pub italic: bool,

    /// Underlined text (SGR 4 / 24).
    pub underline: bool,

    /// Strikethrough text (SGR 9 / 29).
    pub strikethrough: bool,

    /// Active foreground color escape param, e.g. `"38;5;248"`.
    ///
    /// Stored as the bare parameter (without `\x1b[` prefix and `m` suffix) so
    /// the restore sequence can re-emit it generically.
    pub foreground: Option<String>,

    /// Active background color escape param, e.g. `"48;5;248"`.
    ///
    /// Stored as the bare parameter (without `\x1b[` prefix and `m` suffix) so
    /// the restore sequence can re-emit it generically.
    pub background: Option<String>,
}

impl AnsiState {
    /// Returns `true` if any attribute is currently active.
    pub(crate) const fn is_active(&self) -> bool {
        self.bold
            || self.italic
            || self.underline
            || self.strikethrough
            || self.foreground.is_some()
            || self.background.is_some()
    }

    /// Update the tracked state from a complete ANSI escape sequence (e.g.
    /// `"\x1b[1m"`).
    pub(crate) fn update(&mut self, esc: &str) {
        match esc {
            BOLD_START => self.bold = true,
            BOLD_END => self.bold = false,
            ITALIC_START => self.italic = true,
            ITALIC_END => self.italic = false,
            UNDERLINE_START => self.underline = true,
            UNDERLINE_END => self.underline = false,
            STRIKETHROUGH_START => self.strikethrough = true,
            STRIKETHROUGH_END => self.strikethrough = false,
            BG_END => self.background = None,
            FG_END => self.foreground = None,
            RESET => *self = Self::default(),
            _ => {
                // Dynamic color escapes: extract the param between
                // "\x1b[" and "m".
                if let Some(param) = esc.strip_prefix("\x1b[").and_then(|s| s.strip_suffix('m')) {
                    if param.starts_with("48;") {
                        self.background = Some(param.to_string());
                    } else if param.starts_with("38;") {
                        self.foreground = Some(param.to_string());
                    }
                }
            }
        }
    }

    /// Update state by scanning all ANSI escape sequences in `s`.
    pub(crate) fn update_from_str(&mut self, s: &str) {
        for segment in segments(s) {
            if let Segment::Escape(esc) = segment {
                self.update(esc);
            }
        }
    }

    /// Builds a string that re-activates all currently active attributes.
    pub(crate) fn restore_sequence(&self) -> String {
        let mut s = String::new();
        if self.bold {
            s.push_str(BOLD_START);
        }
        if self.italic {
            s.push_str(ITALIC_START);
        }
        if self.underline {
            s.push_str(UNDERLINE_START);
        }
        if self.strikethrough {
            s.push_str(STRIKETHROUGH_START);
        }
        if let Some(fg) = &self.foreground {
            s.push_str("\x1b[");
            s.push_str(fg);
            s.push('m');
        }
        if let Some(bg) = &self.background {
            s.push_str("\x1b[");
            s.push_str(bg);
            s.push('m');
        }
        s
    }
}

/// A lexical segment of a string that may contain ANSI escape sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    /// A run of visible characters (no escape bytes).
    Text(&'a str),

    /// A complete ANSI escape sequence, including the leading `\x1b`.
    /// An unterminated trailing escape is yielded as-is.
    Escape(&'a str),
}

/// Split `s` into visible-text runs and ANSI escape sequences.
///
/// An escape sequence runs from `\x1b` through the first ASCII letter or `~` —
/// sufficient for the SGR/CSI sequences this crate emits and consumes.
/// This is the single tokenizer for every escape-aware routine in the crate
/// (width computation, state tracking, table wrapping), so the termination rule
/// cannot drift between call sites.
pub const fn segments(s: &str) -> Segments<'_> {
    Segments { rest: s }
}

/// Iterator returned by [`segments`].
pub struct Segments<'a> {
    /// Remaining unscanned input.
    rest: &'a str,
}

impl<'a> Iterator for Segments<'a> {
    type Item = Segment<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }

        if let Some(after_esc) = self.rest.strip_prefix('\x1b') {
            let end = after_esc
                .char_indices()
                .find(|&(_, c)| c.is_ascii_alphabetic() || c == '~')
                .map_or(self.rest.len(), |(idx, c)| 1 + idx + c.len_utf8());
            let (escape, rest) = self.rest.split_at(end);
            self.rest = rest;
            return Some(Segment::Escape(escape));
        }

        let end = self.rest.find('\x1b').unwrap_or(self.rest.len());
        let (text, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(Segment::Text(text))
    }
}

/// Calculate the visual width of a string, ignoring ANSI escape sequences.
///
/// Strips ANSI escape sequences (via the shared [`segments`] scanner), then
/// delegates to `UnicodeWidthStr::width()` on the contiguous visible text.
/// Measuring the visible text as a unit is what lets multi-codepoint sequences
/// (emoji presentation via VS16, ZWJ sequences, script-specific ligatures)
/// width correctly even when an escape sits between a base character and its
/// combining mark.
pub fn visual_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr as _;

    let mut plain = String::new();
    for segment in segments(s) {
        if let Segment::Text(text) = segment {
            plain.push_str(text);
        }
    }
    plain.width()
}

#[cfg(test)]
#[path = "ansi_tests.rs"]
mod tests;
