//! ANSI-aware terminal renderer for CommonMark ASTs.
//!
//! This module provides a custom formatter that walks a comrak AST and produces
//! CommonMark-like output with inline ANSI escape codes for terminal styling.
//!
//! Unlike comrak's built-in `format_commonmark`, this renderer is aware of ANSI
//! escape sequences and excludes them from line-width calculations. This
//! prevents the soft-wrapping logic from splitting escape sequences across line
//! boundaries, which would cause background colors to bleed to the end of
//! terminal lines.
//!
//! # Design
//!
//! The renderer is split into two layers:
//!
//! - [`TerminalWriter`] (in [`writer`](crate::writer)) handles the low-level
//!   output concerns: word-wrapping, ANSI escape tracking, column counting,
//!   and line-fill management.
//!
//! - [`TerminalFormatter`] (this module) walks the comrak AST and drives the
//!   writer, emitting CommonMark syntax and ANSI escapes for each node.

use std::{
    cmp::max,
    fmt::{self, Write},
};

use comrak::{
    Node,
    nodes::{
        ListDelimType, ListType, NodeCodeBlock, NodeHeading, NodeHtmlBlock, NodeLink, NodeList,
        NodeTaskItem, NodeValue,
    },
};
use syntect::highlighting::Theme;

use crate::{
    ansi::{
        BG_END, BOLD_END, BOLD_START, FG_END, ITALIC_END, ITALIC_START, STRIKETHROUGH_END,
        STRIKETHROUGH_START, UNDERLINE_END, UNDERLINE_START,
    },
    format::{DefaultBackground, HrStyle},
    table,
    writer::TerminalWriter,
};

/// SGR: Inline code background (gray) — fallback when no theme is set.
const CODE_BG: &str = "\x1b[48;5;248m";

/// Options controlling how horizontal rules are rendered.
pub struct HrOptions {
    /// The rendering style for horizontal rules.
    pub style: HrStyle,

    /// Actual terminal width, used when `style` is [`HrStyle::Line`].
    pub terminal_width: Option<usize>,
}

/// Format a comrak AST as styled terminal output.
///
/// This is the public entry point, called from [`Formatter::format_terminal`].
pub fn format_terminal(
    root: Node<'_>,
    width: usize,
    table_options: &table::TableOptions,
    hr_options: &HrOptions,
    theme: &Theme,
    default_background: Option<&DefaultBackground>,
    output: &mut dyn Write,
) -> fmt::Result {
    let mut f = TerminalFormatter::new(
        root,
        width,
        table_options,
        hr_options,
        theme,
        default_background,
        output,
    );
    f.format(root)
}

/// ANSI-aware CommonMark formatter for terminal output.
///
/// Walks a comrak AST and drives a [`TerminalWriter`] to produce styled
/// terminal output.
pub struct TerminalFormatter<'a, 'w> {
    /// The current node being formatted.
    node: Node<'a>,

    /// The terminal writer handling output and wrapping.
    writer: TerminalWriter<'w>,

    /// Table formatting options.
    table_options: &'w table::TableOptions,

    /// Horizontal rule rendering options.
    hr_options: &'w HrOptions,

    /// Syntax highlighting theme.
    theme: &'w Theme,

    /// Stack of ordered list start numbers.
    ol_stack: Vec<usize>,

    /// Depth of blockquote nesting (for foreground color restore).
    blockquote_depth: usize,

    /// SGR parameter and escape for blockquote foreground color.
    blockquote_fg: (String, String),
}

impl<'a, 'w> TerminalFormatter<'a, 'w> {
    /// Create a new terminal formatter.
    pub fn new(
        node: Node<'a>,
        width: usize,
        table_options: &'w table::TableOptions,
        hr_options: &'w HrOptions,
        theme: &'w Theme,
        default_background: Option<&DefaultBackground>,
        output: &'w mut dyn Write,
    ) -> Self {
        Self {
            node,
            writer: TerminalWriter::new(output, width, default_background),
            table_options,
            hr_options,
            theme,
            ol_stack: vec![],
            blockquote_depth: 0,
            blockquote_fg: theme_blockquote_fg(theme),
        }
    }

    /// Format the entire document.
    pub fn format(&mut self, root: Node<'a>) -> fmt::Result {
        enum Phase {
            /// Pre-order (entering).
            Pre,

            /// Post-order (exiting).
            Post,
        }

        // Emit the default background escape at the very start.
        if let Some(bg) = self.writer.default_background {
            self.writer
                .write_escape(&format!("\x1b[48;5;{}m", bg.color))?;
        }
        let mut stack = vec![(root, Phase::Pre)];

        while let Some((node, phase)) = stack.pop() {
            match phase {
                Phase::Pre => {
                    if self.format_node(node, true)? {
                        stack.push((node, Phase::Post));
                        for ch in node.reverse_children() {
                            stack.push((ch, Phase::Pre));
                        }
                    }
                }
                Phase::Post => {
                    self.format_node(node, false)?;
                }
            }
        }

        self.writer.finish()
    }

    /// Format a single node (entering or exiting).
    ///
    /// Returns `true` if children should be visited (only relevant for
    /// entering).
    fn format_node(&mut self, node: Node<'a>, entering: bool) -> Result<bool, fmt::Error> {
        self.node = node;

        // Track tight list context.
        let parent_node = node.parent();
        if entering {
            if parent_node.is_some_and(|p| {
                matches!(
                    p.data().value,
                    NodeValue::Item(..) | NodeValue::TaskItem(..)
                )
            }) {
                self.writer.in_tight_list_item = get_in_tight_list_item(node);
            }
        } else if matches!(node.data().value, NodeValue::List(..)) {
            self.writer.in_tight_list_item = parent_node.is_some_and(|p| {
                matches!(
                    p.data().value,
                    NodeValue::Item(..) | NodeValue::TaskItem(..)
                )
            }) && get_in_tight_list_item(node);
        }

        let next_is_block = node
            .next_sibling()
            .is_none_or(|next| next.data().value.block());

        match node.data().value {
            NodeValue::BlockQuote => self.format_block_quote(entering)?,
            NodeValue::List(..) => self.format_list(node, entering)?,
            NodeValue::Item(..) => self.format_item(node, entering)?,
            NodeValue::Heading(nh) => self.format_heading(nh, entering)?,
            NodeValue::CodeBlock(ref ncb) => self.format_code_block(node, ncb, entering)?,
            NodeValue::HtmlBlock(ref nhb) => self.format_html_block(nhb, entering)?,
            NodeValue::ThematicBreak => self.format_thematic_break(entering)?,
            NodeValue::Paragraph => self.format_paragraph(entering),
            NodeValue::Text(ref literal) => self.format_text(literal, entering)?,
            NodeValue::LineBreak => self.format_line_break(entering, next_is_block)?,
            NodeValue::SoftBreak => self.format_soft_break(entering)?,
            NodeValue::Code(ref code) => self.format_code(&code.literal, entering)?,
            NodeValue::HtmlInline(ref literal) => self.format_html_inline(literal, entering)?,
            NodeValue::Raw(ref literal) => self.format_raw(literal, entering)?,
            NodeValue::Strong => {
                if parent_node.is_none_or(|p| !matches!(p.data().value, NodeValue::Strong)) {
                    self.format_strong(entering)?;
                }
            }
            NodeValue::Emph => self.format_emph(node, entering)?,
            NodeValue::TaskItem(ref nti) => self.format_task_item(nti, node, entering)?,
            NodeValue::Strikethrough => self.format_strikethrough(entering)?,
            NodeValue::Underline => self.format_underline(entering)?,
            NodeValue::Link(ref nl) => return self.format_link(node, nl, entering),
            NodeValue::Image(ref nl) => self.format_image(nl, entering)?,
            NodeValue::Table(..) => return self.format_table_aligned(node, entering),
            NodeValue::FrontMatter(ref literal) => {
                if entering {
                    self.writer.output(literal, false)?;
                }
            }
            NodeValue::Math(ref math) => {
                if entering {
                    let delim = if math.dollar_math {
                        if math.display_math { "$$" } else { "$" }
                    } else {
                        "$`"
                    };
                    let end_delim = if delim == "$`" { "`$" } else { delim };
                    self.writer.output(delim, false)?;
                    self.writer.output(&math.literal, false)?;
                    self.writer.output(end_delim, false)?;
                }
            }
            NodeValue::FootnoteDefinition(ref nfd) => {
                if entering {
                    self.writer.output(&format!("[^{}]: ", nfd.name), false)?;
                }
            }
            NodeValue::FootnoteReference(ref nfr) => {
                if entering {
                    self.writer.output(&format!("[^{}]", nfr.name), false)?;
                }
            }
            NodeValue::WikiLink(ref nl) => {
                if entering {
                    self.writer.output("[[", false)?;
                } else {
                    self.writer.output("|", false)?;
                    self.writer.output(&nl.url, false)?;
                    self.writer.output("]]", false)?;
                }
            }
            NodeValue::EscapedTag(tag) => {
                if entering {
                    self.writer.output(tag, false)?;
                }
            }
            _ => {}
        }
        Ok(true)
    }

    /// Format a blockquote node.
    fn format_block_quote(&mut self, entering: bool) -> fmt::Result {
        if entering {
            self.blockquote_depth += 1;
            let (ref param, ref escape) = self.blockquote_fg;
            self.writer.attrs.foreground = Some(param.clone());
            self.writer.write_escape(escape)?;
            self.writer.output("> ", false)?;
            self.writer.begin_content = true;
            write!(self.writer.prefix, "> ")?;

            return Ok(());
        }

        let new_len = self.writer.prefix.len().saturating_sub(2);
        self.writer.prefix.truncate(new_len);
        self.blockquote_depth -= 1;
        if self.blockquote_depth > 0 {
            let (ref param, ref escape) = self.blockquote_fg;
            self.writer.attrs.foreground = Some(param.clone());
            self.writer.write_escape(escape)?;
        } else {
            self.writer.attrs.foreground = None;
            self.writer.write_escape(FG_END)?;
        }
        self.writer.blankline();

        Ok(())
    }

    /// Format a list node.
    fn format_list(&mut self, node: Node<'a>, entering: bool) -> fmt::Result {
        let ol_start = match node.data().value {
            NodeValue::List(NodeList {
                list_type: ListType::Ordered,
                start,
                ..
            }) => Some(start),
            _ => None,
        };

        if entering {
            if let Some(start) = ol_start {
                self.ol_stack.push(start);
            }

            return Ok(());
        }

        if ol_start.is_some() {
            self.ol_stack.pop();
        }
        if node.next_sibling().is_some_and(|next| {
            matches!(
                next.data().value,
                NodeValue::CodeBlock(..) | NodeValue::List(..)
            )
        }) {
            self.writer.cr();
            self.writer.output("<!-- end list -->", false)?;
            self.writer.blankline();
        }

        Ok(())
    }

    /// Format a list item node.
    fn format_item(&mut self, node: Node<'a>, entering: bool) -> fmt::Result {
        let parent = match node.parent().expect("item must have parent").data().value {
            NodeValue::List(ref nl) => *nl,
            _ => unreachable!(),
        };

        let marker_width = if parent.list_type == ListType::Bullet {
            2
        } else {
            let mut listmarker = String::new();
            let list_number = self.ol_stack.last_mut().map_or(parent.start, |last_stack| {
                let n = *last_stack;
                if entering {
                    *last_stack += 1;
                }
                n
            });
            let delim = if parent.delimiter == ListDelimType::Paren {
                ")"
            } else {
                "."
            };
            write!(listmarker, "{list_number}{delim} ")?;
            listmarker.len()
        };

        if entering {
            if parent.list_type == ListType::Bullet {
                self.writer.output("- ", false)?;
            } else {
                let list_number = self
                    .ol_stack
                    .last()
                    .map_or(parent.start, |last| last.saturating_sub(1));
                let delim = if parent.delimiter == ListDelimType::Paren {
                    ")"
                } else {
                    "."
                };
                let marker = format!("{list_number}{delim} ");
                self.writer.output(&marker, false)?;
            }
            self.writer.begin_content = true;
            for _ in 0..marker_width {
                write!(self.writer.prefix, " ")?;
            }

            return Ok(());
        }

        let new_len = self.writer.prefix.len().saturating_sub(marker_width);
        self.writer.prefix.truncate(new_len);
        self.writer.cr();

        Ok(())
    }

    /// Format a heading node.
    fn format_heading(&mut self, nh: NodeHeading, entering: bool) -> fmt::Result {
        if entering {
            for _ in 0..nh.level {
                self.writer.output("#", false)?;
            }
            self.writer.output(" ", false)?;
            self.writer.begin_content = true;
            self.writer.no_linebreaks = true;
            self.writer.attrs.bold = true;
            self.writer.write_escape(BOLD_START)?;

            return Ok(());
        }

        self.writer.write_escape(BOLD_END)?;
        self.writer.attrs.bold = false;
        self.writer.no_linebreaks = false;
        self.writer.blankline();

        Ok(())
    }

    /// Format a fenced code block node.
    fn format_code_block(
        &mut self,
        node: Node<'a>,
        ncb: &NodeCodeBlock,
        entering: bool,
    ) -> fmt::Result {
        if !entering {
            return Ok(());
        }

        let first_in_list_item = node.previous_sibling().is_none()
            && node.parent().is_some_and(|p| {
                matches!(
                    p.data().value,
                    NodeValue::Item(..) | NodeValue::TaskItem(..)
                )
            });

        if !first_in_list_item {
            self.writer.blankline();
        }

        let info = &ncb.info;
        let literal = &ncb.literal;
        let fence_byte = if info.contains('`') { '~' } else { '`' };
        let numticks = max(
            3,
            longest_byte_sequence(literal.as_bytes(), fence_byte as u8) + 1,
        );

        // Opening fence + language tag.
        for _ in 0..numticks {
            self.writer.output(&fence_byte.to_string(), false)?;
        }
        if !info.is_empty() {
            self.writer.output(info, false)?;
        }
        self.writer.cr();

        // Content — try syntax highlighting, fall back to plain text.
        if let Some(highlighted) = highlight_code_block(literal, info, self.theme) {
            self.writer.write_raw(&highlighted)?;
        } else {
            self.writer.output(literal, false)?;
        }

        // Closing fence.
        self.writer.cr();
        for _ in 0..numticks {
            self.writer.output(&fence_byte.to_string(), false)?;
        }
        self.writer.blankline();

        Ok(())
    }

    /// Format an HTML block node.
    fn format_html_block(&mut self, nhb: &NodeHtmlBlock, entering: bool) -> fmt::Result {
        if !entering {
            return Ok(());
        }

        self.writer.blankline();
        self.writer.output(&nhb.literal, false)?;
        self.writer.blankline();

        Ok(())
    }

    /// Format a thematic break (`-----`).
    fn format_thematic_break(&mut self, entering: bool) -> fmt::Result {
        if !entering {
            return Ok(());
        }

        self.writer.blankline();
        match self.hr_options.style {
            HrStyle::Markdown => {
                self.writer.output("-----", false)?;
            }
            HrStyle::Line => {
                let line_width = self
                    .hr_options
                    .terminal_width
                    .filter(|&w| w > 0)
                    .unwrap_or(self.writer.width)
                    .max(1);
                let line: String = "─".repeat(line_width);
                self.writer.output(&line, false)?;
            }
        }
        self.writer.blankline();

        Ok(())
    }

    /// Format a paragraph node.
    fn format_paragraph(&mut self, entering: bool) {
        if entering {
            return;
        }

        self.writer.blankline();
    }

    /// Format a text node.
    fn format_text(&mut self, literal: &str, entering: bool) -> fmt::Result {
        if !entering {
            return Ok(());
        }

        self.writer.output(literal, true)?;
        Ok(())
    }

    /// Format a hard line break.
    fn format_line_break(&mut self, entering: bool, next_is_block: bool) -> fmt::Result {
        if !entering || next_is_block {
            return Ok(());
        }

        self.writer.output("\\", false)?;
        self.writer.cr();

        Ok(())
    }

    /// Format a soft line break.
    fn format_soft_break(&mut self, entering: bool) -> fmt::Result {
        if !entering {
            return Ok(());
        }

        if !self.writer.no_linebreaks && self.writer.width == 0 {
            self.writer.cr();
        } else {
            self.writer.output(" ", true)?;
        }

        Ok(())
    }

    /// Format an inline code span with background color.
    fn format_code(&mut self, literal: &str, entering: bool) -> fmt::Result {
        if !entering {
            return Ok(());
        }

        let numticks = shortest_unused_sequence(literal.as_bytes(), b'`');

        let (bg_param, bg_escape) = theme_bg(self.theme);
        self.writer.attrs.background = Some(bg_param);
        self.writer.write_escape(&bg_escape)?;

        for _ in 0..numticks {
            self.writer.output("`", false)?;
        }

        let literal_bytes = literal.as_bytes();
        let all_space = literal_bytes
            .iter()
            .all(|&c| c == b' ' || c == b'\r' || c == b'\n');
        let has_edge_space = !literal_bytes.is_empty()
            && (literal_bytes[0] == b' ' || literal_bytes[literal_bytes.len() - 1] == b' ');
        let has_edge_backtick = !literal_bytes.is_empty()
            && (literal_bytes[0] == b'`' || literal_bytes[literal_bytes.len() - 1] == b'`');

        let pad = literal.is_empty() || has_edge_backtick || (!all_space && has_edge_space);
        if pad {
            self.writer.output(" ", false)?;
        }
        self.writer.output(literal, true)?;
        if pad {
            self.writer.output(" ", false)?;
        }
        for _ in 0..numticks {
            self.writer.output("`", false)?;
        }

        // Restore default background if one is set, otherwise clear.
        if let Some(bg) = self.writer.default_background {
            let param = format!("48;5;{}", bg.color);
            let esc = format!("\x1b[{param}m");
            self.writer.attrs.background = Some(param);
            self.writer.write_escape(&esc)?;
        } else {
            self.writer.write_escape(BG_END)?;
            self.writer.attrs.background = None;
        }

        Ok(())
    }

    /// Format inline HTML.
    fn format_html_inline(&mut self, literal: &str, entering: bool) -> fmt::Result {
        if !entering {
            return Ok(());
        }

        self.writer.output(literal, false)?;
        Ok(())
    }

    /// Format a raw node (pass-through).
    fn format_raw(&mut self, literal: &str, entering: bool) -> fmt::Result {
        if !entering {
            return Ok(());
        }

        self.writer.output(literal, false)?;
        Ok(())
    }

    /// Format bold text with ANSI bold.
    fn format_strong(&mut self, entering: bool) -> fmt::Result {
        if entering {
            self.writer.attrs.bold = true;
            self.writer.write_escape(BOLD_START)?;
        }

        self.writer.output("**", false)?;

        if !entering {
            self.writer.write_escape(BOLD_END)?;
            self.writer.attrs.bold = false;
        }

        Ok(())
    }

    /// Format emphasized text with ANSI italic.
    fn format_emph(&mut self, node: Node<'a>, entering: bool) -> fmt::Result {
        let emph_delim = if node
            .parent()
            .is_some_and(|p| matches!(p.data().value, NodeValue::Emph))
            && node.next_sibling().is_none()
            && node.previous_sibling().is_none()
        {
            "_"
        } else {
            "*"
        };

        if entering {
            self.writer.attrs.italic = true;
            self.writer.write_escape(ITALIC_START)?;
        }

        self.writer.output(emph_delim, false)?;

        if !entering {
            self.writer.write_escape(ITALIC_END)?;
            self.writer.attrs.italic = false;
        }

        Ok(())
    }

    /// Format a task list item with checkbox.
    fn format_task_item(
        &mut self,
        nti: &NodeTaskItem,
        node: Node<'a>,
        entering: bool,
    ) -> fmt::Result {
        if node
            .parent()
            .is_some_and(|p| matches!(p.data().value, NodeValue::List(_)))
        {
            self.format_item(node, entering)?;
        }

        if entering {
            let sym = nti.symbol.unwrap_or(' ');
            self.writer.output(&format!("[{sym}] "), false)?;
        }

        Ok(())
    }

    /// Format strikethrough text.
    fn format_strikethrough(&mut self, entering: bool) -> fmt::Result {
        if entering {
            self.writer.attrs.strikethrough = true;
            self.writer.write_escape(STRIKETHROUGH_START)?;
        }

        self.writer.output("~~", false)?;

        if !entering {
            self.writer.write_escape(STRIKETHROUGH_END)?;
            self.writer.attrs.strikethrough = false;
        }

        Ok(())
    }

    /// Format underlined text.
    fn format_underline(&mut self, entering: bool) -> fmt::Result {
        if entering {
            self.writer.attrs.underline = true;
            self.writer.write_escape(UNDERLINE_START)?;
        }

        self.writer.output("__", false)?;

        if !entering {
            self.writer.write_escape(UNDERLINE_END)?;
            self.writer.attrs.underline = false;
        }

        Ok(())
    }

    /// Format a link (autolink or inline).
    fn format_link(
        &mut self,
        node: Node<'a>,
        nl: &NodeLink,
        entering: bool,
    ) -> Result<bool, fmt::Error> {
        if is_autolink(node, nl) {
            if entering {
                let url = nl.url.strip_prefix("mailto:").unwrap_or(&nl.url);
                self.writer.output("<", false)?;
                self.writer.output(url, false)?;
                self.writer.output(">", false)?;
                return Ok(false);
            }
        } else if entering {
            self.writer.output("[", false)?;
        } else {
            self.writer.output("](", false)?;
            self.writer.output(&nl.url, false)?;
            if !nl.title.is_empty() {
                self.writer.output(" \"", false)?;
                self.writer.output(&nl.title, false)?;
                self.writer.output("\"", false)?;
            }
            self.writer.output(")", false)?;
        }

        Ok(true)
    }

    /// Format an image.
    fn format_image(&mut self, nl: &NodeLink, entering: bool) -> fmt::Result {
        if entering {
            self.writer.output("![", false)?;
        } else {
            self.writer.output("](", false)?;
            self.writer.output(&nl.url, false)?;
            if !nl.title.is_empty() {
                self.writer.output(" \"", false)?;
                self.writer.output(&nl.title, false)?;
                self.writer.output("\"", false)?;
            }
            self.writer.output(")", false)?;
        }

        Ok(())
    }

    /// Format a table with aligned columns.
    fn format_table_aligned(&mut self, node: Node<'a>, entering: bool) -> Result<bool, fmt::Error> {
        if !entering {
            self.writer.blankline();
            return Ok(false);
        }

        self.writer.blankline();

        if let Some(rendered) = table::format_table(
            node,
            self.table_options,
            self.hr_options,
            self.theme,
            self.writer.default_background.as_ref(),
        ) {
            self.writer.output(&rendered, false)?;
        }

        Ok(false)
    }
}

impl Write for TerminalFormatter<'_, '_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.writer.output(s, false)
    }
}

/// Check if the current node is in a tight list item.
fn get_in_tight_list_item(node: Node<'_>) -> bool {
    let mut current = Some(node);
    while let Some(n) = current {
        match n.data().value {
            NodeValue::Item(..) | NodeValue::TaskItem(..) => {
                if let Some(list_node) = n.parent()
                    && let NodeValue::List(ref nl) = list_node.data().value
                {
                    return nl.tight;
                }
                return false;
            }
            _ => {
                current = n.parent();
            }
        }
    }

    false
}

/// Check if a link node is an autolink.
fn is_autolink(node: Node<'_>, nl: &NodeLink) -> bool {
    if nl.url.is_empty() || !nl.title.is_empty() {
        return false;
    }

    let link_text = match node.first_child() {
        None => return false,
        Some(child) => match child.data().value {
            NodeValue::Text(ref t) => t.clone(),
            _ => return false,
        },
    };

    let url = nl.url.strip_prefix("mailto:").unwrap_or(&nl.url);
    url == link_text.as_ref()
}

/// Find the longest consecutive sequence of `ch` in `buffer`.
fn longest_byte_sequence(buffer: &[u8], ch: u8) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for &c in buffer {
        if c == ch {
            current += 1;
        } else {
            if current > longest {
                longest = current;
            }
            current = 0;
        }
    }

    max(longest, current)
}

/// Find the shortest sequence length of `f` not present in `buffer`.
fn shortest_unused_sequence(buffer: &[u8], f: u8) -> usize {
    let mut used = std::collections::HashSet::new();
    let mut current = 0;
    for &c in buffer {
        if c == f {
            current += 1;
        } else {
            if current > 0 {
                used.insert(current);
            }
            current = 0;
        }
    }

    if current > 0 {
        used.insert(current);
    }

    let mut i = 1;
    while used.contains(&i) {
        i += 1;
    }

    i
}

/// Returns the SGR parameter string and the full ANSI escape for the theme's
/// background color.
///
/// Falls back to 8-bit gray (color 248) when the theme has no background.
pub fn theme_bg(theme: &Theme) -> (String, String) {
    theme.settings.background.map_or_else(
        || ("48;5;248".to_string(), CODE_BG.to_string()),
        |color| {
            let param = format!("48;2;{};{};{}", color.r, color.g, color.b);
            let escape = format!("\x1b[{param}m");
            (param, escape)
        },
    )
}

/// Returns the SGR parameter and escape for blockquote foreground color.
///
/// Uses the theme's gutter foreground color if available, falling back to
/// 8-bit gray (color 248).
fn theme_blockquote_fg(theme: &Theme) -> (String, String) {
    theme.settings.gutter_foreground.map_or_else(
        || ("38;5;248".to_string(), "\x1b[38;5;248m".to_string()),
        |color| {
            let param = format!("38;2;{};{};{}", color.r, color.g, color.b);
            let escape = format!("\x1b[{param}m");
            (param, escape)
        },
    )
}

/// Syntax-highlight a code block using `syntect`.
///
/// Returns `Some(highlighted)` on success, or `None` if the language is
/// empty or not recognized. The caller should fall back to plain rendering.
fn highlight_code_block(literal: &str, language: &str, theme: &Theme) -> Option<String> {
    if language.is_empty() {
        return None;
    }

    let ss = two_face::syntax::extra_newlines();
    let syntax = ss.find_syntax_by_token(language)?;

    let mut h = syntect::easy::HighlightLines::new(syntax, theme);
    let mut buf = String::new();

    for line in syntect::util::LinesWithEndings::from(literal) {
        let ranges = h.highlight_line(line, &ss).ok()?;
        let escaped = syntect::util::as_24_bit_terminal_escaped(&ranges, false);
        buf.push_str(&escaped);
    }

    // Reset colors at the end of the block.
    buf.push_str("\x1b[0m");
    Some(buf)
}
