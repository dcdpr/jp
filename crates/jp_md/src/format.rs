//! Markdown formatting utilities.

use std::{borrow::Cow, fmt, sync::LazyLock};

use comrak::{
    Arena,
    nodes::{NodeList, NodeValue},
    options::{Extension, ListStyleType, Render},
};
use syntect::{highlighting::Theme, parsing::SyntaxSet};
use two_face::syntax;

use crate::{
    ansi,
    render::{self, HrOptions, RenderOptions},
    table::TableOptions,
    theme,
};

/// Process-wide syntax set, deserialized once and shared across all code
/// highlighting.
///
/// `two_face::syntax::extra_newlines` rebuilds the whole `SyntaxSet` from its
/// embedded dump on each call and returns it with cold, lazily-compiled regex
/// caches.
/// Sharing a single instance keeps syntect's grammar regexes compiled once for
/// the life of the process instead of recompiling them for every code line.
pub(crate) static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(syntax::extra_newlines);

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

    /// Render a continuous unicode horizontal line (`─`) spanning the full
    /// line width (based on `wrap_width`).
    #[default]
    Line,
}

/// A default background color applied to all content, with a fill mode
/// controlling how far it extends on each line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultBackground {
    /// SGR background parameter, e.g. `"48;5;236"` or `"48;2;80;73;69"`.
    pub param: String,

    /// How far the background extends on each line.
    pub fill: BackgroundFill,
}

/// Per-call options for [`Formatter::format_terminal_with`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TerminalOptions {
    /// Default background color applied to all content in this block.
    ///
    /// When set, the renderer applies this background and restores it after
    /// inline elements (like code spans) that set their own background.
    pub default_background: Option<DefaultBackground>,

    /// Visual indent (in spaces) applied to wrap-routed rendered content.
    ///
    /// The streaming buffer sets this when emitting events from inside a nested
    /// list item, so the renderer puts paragraphs, headers, list items,
    /// blockquotes, inline-code wrap continuations, and code-block bodies at
    /// the correct visual column.
    /// `0` (the default) means no indent — content renders at column 0.
    pub indent: usize,

    /// When `true`, omit the trailing inter-block separator that this call
    /// would otherwise append after the rendered content.
    ///
    /// The separator is the blank line that visually divides consecutive
    /// blocks.
    /// Suppressing it lets a streaming caller emit the separation itself once
    /// it knows what follows — for instance, to decide whether the gap between
    /// two blocks carries a background.
    pub suppress_trailing_separator: bool,

    /// When `true`, emit the trailing separator even after a tight list.
    ///
    /// The tight-list suppression keeps streamed mid-list items visually tight.
    /// A caller flushing the *final* block of a region — the list is over —
    /// sets this so the terminal item still gets its separator, instead of
    /// gluing the next content directly onto the last item.
    pub force_trailing_separator: bool,
}

/// A formatter for markdown text.
pub struct Formatter {
    /// Target line width for wrapping.
    /// `0` disables wrapping.
    width: usize,

    /// Maximum visual width for a single table column.
    /// `0` = unlimited.
    table_max_column_width: usize,

    /// Resolved syntax highlighting theme.
    theme: Theme,

    /// How horizontal rules are rendered in terminal output.
    hr_style: HrStyle,

    /// Width of the output terminal in columns, if known.
    ///
    /// Bounds blocks that are laid out rather than wrapped, so they don't run
    /// into the terminal's own line wrapping.
    /// Tables are fitted to it; when `None` they keep their natural widths,
    /// which is what piped and redirected output wants.
    terminal_width: Option<usize>,

    /// Override background color for inline code spans.
    ///
    /// When set, inline code uses this color instead of the theme's background.
    /// Stored as a pre-resolved `(sgr_param, full_escape)` pair.
    inline_code_bg: Option<(String, String)>,
}

impl fmt::Debug for Formatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Formatter")
            .field("width", &self.width)
            .field("table_max_column_width", &self.table_max_column_width)
            .field("theme", &"<syntect::Theme>")
            .field("hr_style", &self.hr_style)
            .field("terminal_width", &self.terminal_width)
            .field("inline_code_bg", &self.inline_code_bg)
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
            inline_code_bg: None,
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
            inline_code_bg: None,
        }
    }

    /// Set the maximum table column width.
    #[must_use]
    pub const fn table_max_column_width(mut self, width: usize) -> Self {
        self.table_max_column_width = width;
        self
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: Option<&str>) -> Self {
        self.theme = theme::resolve(theme);
        self
    }

    /// Set the HR style.
    #[must_use]
    pub const fn pretty_hr(mut self, pretty: bool) -> Self {
        self.hr_style = if pretty {
            HrStyle::Line
        } else {
            HrStyle::Markdown
        };
        self
    }

    /// Set the width of the output terminal in columns.
    ///
    /// Tables are laid out to fit this width, narrowing their widest columns
    /// when the natural layout would be wider.
    /// A width of `0` is treated as unknown, leaving tables unbounded.
    #[must_use]
    pub const fn terminal_width(mut self, width: usize) -> Self {
        self.terminal_width = if width > 0 { Some(width) } else { None };
        self
    }

    /// Override the background color for inline code spans.
    ///
    /// When set, inline code uses this color instead of the theme's background.
    /// The color is pre-resolved to an SGR `(param, escape)` pair.
    #[must_use]
    pub fn inline_code_bg(mut self, param: Option<String>) -> Self {
        self.inline_code_bg = param.map(|p| {
            let escape = format!("\x1b[{p}m");
            (p, escape)
        });
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
        self.render_terminal(text, &TerminalOptions::default(), false)
    }

    /// Like [`format_terminal`], but with per-call options that control
    /// rendering behaviour for this specific block.
    ///
    /// # Errors
    ///
    /// Returns an error if `fmt::Error` is returned when formatting the
    /// markdown.
    ///
    /// [`format_terminal`]: Self::format_terminal
    pub fn format_terminal_with(
        &self,
        text: &str,
        options: &TerminalOptions,
    ) -> Result<String, fmt::Error> {
        self.render_terminal(text, options, true)
    }

    /// Core terminal rendering.
    /// When `auto_separator` is true, appends an inter-block blank line unless
    /// the AST ends with a tight list.
    fn render_terminal(
        &self,
        text: &str,
        options: &TerminalOptions,
        auto_separator: bool,
    ) -> Result<String, fmt::Error> {
        let comrak_options = self.parse_options();
        let arena = Arena::new();
        let ast = comrak::parse_document(&arena, text, &comrak_options);
        let table_options = TableOptions::new(self.table_max_column_width);
        let hr_options = HrOptions {
            style: self.hr_style,
        };
        let render_options = RenderOptions {
            width: self.width,
            terminal_width: self.terminal_width,
            table_options: &table_options,
            hr_options: &hr_options,
            theme: &self.theme,
            default_background: options.default_background.as_ref(),
            inline_code_bg: self.inline_code_bg.as_ref(),
            indent: options.indent,
        };

        let mut buf = String::new();
        render::format_terminal(ast, render_options, &mut buf)?;

        // In streaming mode, append inter-block separator. Suppress
        // only for mid-list items: tight list AND no trailing blank
        // line in the source (the buffer ends terminal items with
        // "\n\n" but mid-list items with just "\n").
        let is_mid_list = ends_with_tight_list(ast)
            && !text.ends_with("\n\n")
            && !options.force_trailing_separator;
        if auto_separator && !is_mid_list && !options.suppress_trailing_separator {
            buf.push_str(&render_separator(options.default_background.as_ref()));
        }

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

    /// Begin a streaming code block for the given language.
    ///
    /// Returns a [`CodeBlockState`] that tracks syntax highlighting across
    /// lines.
    /// Pass it to [`render_code_line`] for each line.
    ///
    /// [`render_code_line`]: Self::render_code_line
    #[must_use]
    pub fn begin_code_block(&self, language: &str) -> CodeBlockState {
        let highlight = self
            .new_code_highlighter(language)
            .map(CodeHighlighter::save);
        CodeBlockState { highlight }
    }

    /// Render a single code line with syntax highlighting and optional
    /// background.
    pub fn render_code_line(
        &self,
        line: &str,
        state: &mut CodeBlockState,
        background: Option<&DefaultBackground>,
    ) -> String {
        let highlighted = if let Some(saved) = state.highlight.take() {
            let mut hl = self.resume_code_highlighter(saved);
            let result = hl.highlight(line).unwrap_or_else(|_| line.to_string());
            state.highlight = Some(hl.save());
            result
        } else {
            line.to_string()
        };
        apply_line_background(&highlighted, background)
    }

    /// Apply optional background to a code fence line.
    #[must_use]
    pub fn render_code_fence(&self, fence: &str, background: Option<&DefaultBackground>) -> String {
        apply_line_background(fence, background)
    }

    /// Render a closing code fence with a trailing blank separator line.
    #[must_use]
    pub fn render_closing_fence(
        &self,
        fence: &str,
        background: Option<&DefaultBackground>,
    ) -> String {
        let mut out = apply_line_background(fence, background);
        out.push_str(&render_separator(background));
        out
    }

    /// Creates a new code highlighter for the given language.
    fn new_code_highlighter(&self, language: &str) -> Option<CodeHighlighter<'_>> {
        let syntax = SYNTAXES.find_syntax_by_token(language)?;
        Some(CodeHighlighter {
            hl: syntect::easy::HighlightLines::new(syntax, &self.theme),
        })
    }

    /// Reconstruct a [`CodeHighlighter`] from previously saved state.
    fn resume_code_highlighter(&self, saved: SavedHighlightState) -> CodeHighlighter<'_> {
        CodeHighlighter::from_saved(&self.theme, saved)
    }
}

/// Opaque state for a streaming code block.
///
/// Created by [`Formatter::begin_code_block`], passed to
/// [`Formatter::render_code_line`] for each line.
/// Tracks syntax highlighting across lines without exposing internal types.
pub struct CodeBlockState {
    /// Saved highlighting state, if the language was recognized.
    highlight: Option<SavedHighlightState>,
}

/// Check if the last top-level node in a comrak AST is a tight list.
///
/// Used to suppress the inter-block separator for streaming mid-list items,
/// which are each rendered as standalone single-item lists.
fn ends_with_tight_list<'a>(root: &'a comrak::nodes::AstNode<'a>) -> bool {
    let Some(last) = root.last_child() else {
        return false;
    };
    matches!(
        last.data().value,
        NodeValue::List(NodeList { tight: true, .. })
    )
}

/// The text that extends a background from `column` to the end of the line.
///
/// The one place [`BackgroundFill`] is interpreted.
/// Every writer that maintains a region background consults this so the three
/// modes cannot drift apart: `Content` adds nothing, `Terminal` defers to the
/// terminal's erase-to-end-of- line, and `Column` pads with real spaces — the
/// only form a host that lays out its own sub-window (an `fzf` preview pane)
/// renders, since it does not implement the erase.
///
/// The caller is responsible for having the background active before writing
/// the result.
pub(crate) fn line_fill(fill: BackgroundFill, column: usize) -> Cow<'static, str> {
    match fill {
        BackgroundFill::Content => Cow::Borrowed(""),
        BackgroundFill::Terminal => Cow::Borrowed("\x1b[K"),
        BackgroundFill::Column(target) => match target.saturating_sub(column) {
            0 => Cow::Borrowed(""),
            pad => Cow::Owned(" ".repeat(pad)),
        },
    }
}

/// Render an inter-block separator (blank line) with optional background fill.
/// Used between blocks and after closing code fences.
#[must_use]
pub fn render_separator(background: Option<&DefaultBackground>) -> String {
    let Some(bg) = background else {
        return "\n".to_string();
    };

    // The separator line holds no content, so the fill starts at column 0.
    let fill = line_fill(bg.fill, 0);
    if fill.is_empty() {
        return "\n".to_string();
    }

    format!("\x1b[{}m{fill}\x1b[49m\n", bg.param)
}

/// Apply an optional default background to content, injecting the background
/// escape at the start of each line and line-fill before each newline.
#[must_use]
pub fn apply_line_background(content: &str, background: Option<&DefaultBackground>) -> String {
    let Some(bg) = background else {
        return content.to_string();
    };
    let bg_esc = format!("\x1b[{}m", bg.param);

    let lines: Vec<&str> = content.split('\n').collect();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str("\x1b[0m\n");
        }
        out.push_str(&bg_esc);
        out.push_str(line);

        // Only a line that a newline completes gets a fill; the trailing
        // segment after the final newline isn't a line.
        // Width is measured per line because the content arrives already
        // syntax-highlighted, so its escapes must not count toward the column
        // the fill starts from.
        if i + 1 < lines.len() {
            out.push_str(&line_fill(bg.fill, ansi::visual_width(line)));
        }
    }
    out
}

/// Saved state from a [`CodeHighlighter`], allowing it to be suspended and
/// resumed across borrow boundaries.
struct SavedHighlightState {
    /// The syntect highlight state (styling context).
    highlight_state: syntect::highlighting::HighlightState,
    /// The syntect parse state (grammar context).
    parse_state: syntect::parsing::ParseState,
}

/// A stateful syntax highlighter for code blocks.
struct CodeHighlighter<'a> {
    /// The syntect highlighter.
    hl: syntect::easy::HighlightLines<'a>,
}

impl<'a> CodeHighlighter<'a> {
    /// Highlight a single line of code.
    fn highlight(&mut self, line: &str) -> Result<String, syntect::Error> {
        let ranges = self.hl.highlight_line(line, &SYNTAXES)?;
        let mut escaped = syntect::util::as_24_bit_terminal_escaped(&ranges, false);
        escaped.push_str("\x1b[0m");
        Ok(escaped)
    }

    /// Decompose into owned state that can be stored without borrowing.
    fn save(self) -> SavedHighlightState {
        let (highlight_state, parse_state) = self.hl.state();
        SavedHighlightState {
            highlight_state,
            parse_state,
        }
    }

    /// Reconstruct from previously saved state.
    fn from_saved(theme: &'a Theme, saved: SavedHighlightState) -> Self {
        Self {
            hl: syntect::easy::HighlightLines::from_state(
                theme,
                saved.highlight_state,
                saved.parse_state,
            ),
        }
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
