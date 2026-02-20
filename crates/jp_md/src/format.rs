//! Markdown formatting utilities.

use std::fmt;

use comrak::{
    Arena,
    options::{Extension, ListStyleType, Render},
};
use syntect::highlighting::Theme;
use two_face::syntax;

use crate::{
    render::{self, HrOptions},
    table::TableOptions,
    theme,
};

/// Default wrap width for terminal output.
const DEFAULT_WIDTH: usize = 80;

/// Default maximum column width for tables.
const DEFAULT_TABLE_MAX_COL_WIDTH: usize = 40;

/// How a default background color fills each line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundFill {
    /// Fill to the last visible character on the line.
    Content,

    /// Fill to a fixed column width (padding with spaces if needed).
    Column(usize),

    /// Fill to the end of the terminal window via `\x1b[K`.
    Terminal,
}

/// Controls how horizontal rules (`---`) are rendered in terminal output.
#[derive(Debug, Clone, Copy, Default)]
pub enum HrStyle {
    /// Render the original markdown (`---`).
    Markdown,

    /// Render a continuous unicode horizontal line (`─`) spanning the full line
    /// width (based on `wrap_width`).
    #[default]
    Line,
}

/// A default background color applied to all content, with a fill mode
/// controlling how far it extends on each line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultBackground {
    /// ANSI 256-color index.
    pub color: u8,

    /// How far the background extends on each line.
    pub fill: BackgroundFill,
}

/// Per-call options for [`Formatter::format_terminal_with`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    /// Default background color applied to all content in this block.
    ///
    /// When set, the renderer applies this background and restores it after
    /// inline elements (like code spans) that set their own background.
    pub default_background: Option<DefaultBackground>,
}

/// A formatter for markdown text.
pub struct Formatter {
    /// Target line width for wrapping. `0` disables wrapping.
    width: usize,

    /// Maximum visual width for a single table column. `0` = unlimited.
    table_max_column_width: usize,

    /// Resolved syntax highlighting theme.
    theme: Theme,

    /// How horizontal rules are rendered in terminal output.
    hr_style: HrStyle,

    /// Actual terminal width in columns, if known.
    ///
    /// Used by [`HrStyle::Line`] to render a horizontal line spanning the full
    /// terminal width. When `None`, the configured `width` is used as a
    /// fallback.
    terminal_width: Option<usize>,
}

impl fmt::Debug for Formatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Formatter")
            .field("width", &self.width)
            .field("table_max_column_width", &self.table_max_column_width)
            .field("theme", &"<syntect::Theme>")
            .field("hr_style", &self.hr_style)
            .field("terminal_width", &self.terminal_width)
            .finish()
    }
}

impl Default for Formatter {
    fn default() -> Self {
        Self::new()
    }
}

impl Formatter {
    /// Create a new formatter with the default wrap width (80 columns).
    #[must_use]
    pub fn new() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            table_max_column_width: DEFAULT_TABLE_MAX_COL_WIDTH,
            theme: theme::resolve(None),
            hr_style: HrStyle::default(),
            terminal_width: None,
        }
    }

    /// Create a new formatter with the given wrap width.
    ///
    /// A width of `0` disables wrapping entirely.
    #[must_use]
    pub fn with_width(width: usize) -> Self {
        Self {
            width,
            table_max_column_width: DEFAULT_TABLE_MAX_COL_WIDTH,
            theme: theme::resolve(None),
            hr_style: HrStyle::default(),
            terminal_width: None,
        }
    }

    /// Set the maximum table column width.
    #[must_use]
    pub const fn table_max_column_width(mut self, width: usize) -> Self {
        self.table_max_column_width = width;
        self
    }

    /// Set the actual terminal width in columns.
    ///
    /// When [`HrStyle::Line`] is active, horizontal rules are rendered as
    /// a unicode line spanning this width. If not set, the configured
    /// `width` is used instead.
    #[must_use]
    pub const fn terminal_width(mut self, width: usize) -> Self {
        self.terminal_width = Some(width);
        self
    }

    /// Format the markdown into a consistent style.
    ///
    /// # Errors
    ///
    /// Returns an error if `fmt::Error` is returned when formatting the
    /// markdown.
    pub fn format(&self, text: &str) -> Result<String, fmt::Error> {
        self.format_commonmark(text)
    }

    /// Similar to [`Self::format`], but for terminal output.
    ///
    /// This injects ANSI escape codes into the text, to make certain markdown
    /// elements reflect their style (strong, italics, code, color, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if `fmt::Error` is returned when formatting the
    /// markdown.
    pub fn format_terminal(&self, text: &str) -> Result<String, fmt::Error> {
        self.format_terminal_with(text, &TerminalOptions::default())
    }

    /// Like [`format_terminal`](Self::format_terminal), but with per-call
    /// options that control rendering behaviour for this specific block.
    ///
    /// # Errors
    ///
    /// Returns an error if `fmt::Error` is returned when formatting the
    /// markdown.
    pub fn format_terminal_with(
        &self,
        text: &str,
        options: &TerminalOptions,
    ) -> Result<String, fmt::Error> {
        let comrak_options = self.parse_options();
        let arena = Arena::new();
        let ast = comrak::parse_document(&arena, text, &comrak_options);
        let table_options = TableOptions::new(self.table_max_column_width);
        let hr_options = HrOptions {
            style: self.hr_style,
            terminal_width: self.terminal_width,
        };

        let mut buf = String::new();
        render::format_terminal(
            ast,
            self.width,
            &table_options,
            &hr_options,
            &self.theme,
            options.default_background.as_ref(),
            &mut buf,
        )?;
        Ok(buf)
    }

    /// Format the markdown into a consistent style (non-terminal).
    ///
    /// # Errors
    ///
    /// Returns an error if `fmt::Error` is returned when formatting the
    /// markdown.
    fn format_commonmark(&self, text: &str) -> Result<String, fmt::Error> {
        let options = self.parse_options();
        let arena = Arena::new();
        let ast = comrak::parse_document(&arena, text, &options);

        let mut buf = String::new();
        comrak::format_commonmark(ast, &options, &mut buf)?;

        Ok(buf)
    }

    /// Returns the comrak parse/render options used by the formatter.
    fn parse_options(&self) -> comrak::Options<'static> {
        comrak::Options {
            extension: Extension {
                strikethrough: true,
                table: true,
                tasklist: true,
                superscript: true,
                underline: true,
                subscript: true,
                greentext: false,
                ..Default::default()
            },
            render: Render {
                width: self.width,
                list_style: ListStyleType::Dash,
                prefer_fenced: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Creates a new code highlighter for the given language.
    ///
    /// This is useful for streaming code blocks where you want to highlight
    /// each line as it arrives, rather than waiting for the entire block.
    #[must_use]
    pub fn new_code_highlighter(&self, language: &str) -> Option<CodeHighlighter<'_>> {
        let ss = syntax::extra_newlines();
        let syntax = ss.find_syntax_by_token(language)?;
        Some(CodeHighlighter {
            hl: syntect::easy::HighlightLines::new(syntax, &self.theme),
        })
    }
}

/// A stateful syntax highlighter for code blocks.
pub struct CodeHighlighter<'a> {
    /// The syntect highlighter.
    hl: syntect::easy::HighlightLines<'a>,
}

impl CodeHighlighter<'_> {
    /// Highlight a single line of code.
    ///
    /// # Errors
    ///
    /// Returns an error if `syntect::Error` is returned.
    pub fn highlight(&mut self, line: &str) -> Result<String, syntect::Error> {
        let ss = syntax::extra_newlines();
        // highlight_line expects the line to include the newline if one is present.
        let ranges = self.hl.highlight_line(line, &ss)?;
        let mut escaped = syntect::util::as_24_bit_terminal_escaped(&ranges, false);
        escaped.push_str("\x1b[0m");
        Ok(escaped)
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
