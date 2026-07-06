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

    /// Update the tracked state from a complete ANSI SGR escape (e.g.
    /// `"\x1b[1m"` or the compound `"\x1b[1;48;5;236m"`).
    ///
    /// Each `;`-separated SGR sub-parameter is parsed, so attributes combined
    /// into one escape (`\x1b[1;48;5;236m`, `\x1b[0;48;5;236m`, `\x1b[39;49m`)
    /// are all tracked — matching only the leading sub-parameter would miss
    /// every attribute after the first.
    /// Non-SGR escapes (anything not of the form `\x1b[…m`) leave the state
    /// untouched.
    ///
    /// Returns `true` when the escape resets all attributes or sets/clears the
    /// background — the signal a default-background overlay uses to know it
    /// must re-assert its fill after the escape is forwarded.
    pub(crate) fn update(&mut self, esc: &str) -> bool {
        let Some(params) = esc.strip_prefix("\x1b[").and_then(|s| s.strip_suffix('m')) else {
            return false;
        };

        // An empty parameter list (`\x1b[m`) is shorthand for a full reset.
        if params.is_empty() {
            *self = Self::default();
            return true;
        }

        let mut touched_background = false;
        let mut tokens = params.split(';');
        while let Some(code) = tokens.next() {
            match code {
                "0" => {
                    *self = Self::default();
                    touched_background = true;
                }
                "1" => self.bold = true,
                "22" => self.bold = false,
                "3" => self.italic = true,
                "23" => self.italic = false,
                "4" => self.underline = true,
                "24" => self.underline = false,
                "9" => self.strikethrough = true,
                "29" => self.strikethrough = false,
                "39" => self.foreground = None,
                "49" => {
                    self.background = None;
                    touched_background = true;
                }
                "38" => {
                    if let Some(color) = consume_color("38", &mut tokens) {
                        self.foreground = Some(color);
                    }
                }
                "48" => {
                    if let Some(color) = consume_color("48", &mut tokens) {
                        self.background = Some(color);
                    }
                    touched_background = true;
                }
                _ => {}
            }
        }

        touched_background
    }

    /// Update state by scanning all ANSI escape sequences in `s`.
    pub(crate) fn update_from_str(&mut self, s: &str) {
        for segment in segments(s) {
            if let Segment::Escape(esc) = segment {
                let _affects_background = self.update(esc);
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

/// Read the operands of a `38` (foreground) or `48` (background) SGR color
/// introducer, returning the full parameter string (e.g. `"48;5;236"` or
/// `"48;2;80;73;69"`).
///
/// `38`/`48` are followed by either `5;<index>` (8-bit) or `2;<r>;<g>;<b>`
/// (24-bit); those operands are consumed from `tokens` so the surrounding
/// parser resumes at the next attribute.
/// Returns `None` for a malformed introducer, having consumed whatever operands
/// it did read.
fn consume_color<'a, I: Iterator<Item = &'a str>>(prefix: &str, tokens: &mut I) -> Option<String> {
    match tokens.next()? {
        "5" => {
            let index = tokens.next()?;
            Some(format!("{prefix};5;{index}"))
        }
        "2" => {
            let r = tokens.next()?;
            let g = tokens.next()?;
            let b = tokens.next()?;
            Some(format!("{prefix};2;{r};{g};{b}"))
        }
        _ => None,
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
            // OSC string sequences (`\x1b]…`) run to their BEL/ST terminator,
            // not the first letter — so an OSC 8 hyperlink stays a single escape
            // instead of being split across the URL.
            if let Some(body) = after_esc.strip_prefix(']') {
                let end = osc_terminator_end(body).map_or(self.rest.len(), |term| 2 + term);
                let (escape, rest) = self.rest.split_at(end);
                self.rest = rest;
                return Some(Segment::Escape(escape));
            }

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

/// Find the end (exclusive byte offset) of an OSC string terminator within an
/// OSC body — the bytes following `\x1b]`.
///
/// OSC sequences end with either BEL (`\x07`) or ST (`\x1b\\`).
/// Returns `None` when the body holds no terminator yet (an OSC split across a
/// write boundary), so the caller treats the remainder as one unterminated
/// escape.
fn osc_terminator_end(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x07 => return Some(i + 1),
            0x1b if bytes.get(i + 1) == Some(&b'\\') => return Some(i + 2),
            _ => i += 1,
        }
    }
    None
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
