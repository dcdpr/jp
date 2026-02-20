//! Shared ANSI SGR escape constants and state tracking.
//!
//! This module provides the escape sequences, state tracking, and visual
//! width computation used by both the terminal renderer (`render.rs`) and
//! the table formatter (`table.rs`).

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
/// Used to close formatting at line breaks and re-open it on the next
/// line, both for the terminal renderer's incremental wrapping and the
/// table formatter's batch wrapping.
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

    /// Update the tracked state from a complete ANSI escape sequence
    /// (e.g. `"\x1b[1m"`).
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
        let mut in_escape = false;
        let mut esc = String::new();
        for c in s.chars() {
            if in_escape {
                esc.push(c);
                if c.is_ascii_alphabetic() || c == '~' {
                    in_escape = false;
                    self.update(&esc);
                    esc.clear();
                }
            } else if c == '\x1b' {
                in_escape = true;
                esc.clear();
                esc.push(c);
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

/// Calculate the visual width of a string, ignoring ANSI escape sequences.
///
/// Uses Unicode width rules (UAX #11) so that wide characters such as CJK
/// ideographs and emoji are correctly counted as 2 columns. Control characters
/// and escape sequences contribute zero width.
pub fn visual_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar as _;

    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c.is_ascii_alphabetic() || c == '~' {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            len += c.width().unwrap_or(0);
        }
    }
    len
}

#[cfg(test)]
#[path = "ansi_tests.rs"]
mod tests;
