//! Custom node formatter for comrak's HTML renderer.
//!
//! Delegates structural elements to comrak and overrides text nodes, code
//! spans, code blocks, raw HTML, and alerts so that every rendered text run is
//! wrapped in a [`span`], and headings so that they carry their anchors.

use std::{
    collections::BTreeMap,
    fmt::{self, Write as _},
};

use comrak::{
    html::{ChildRendering, Context, format_node_default},
    nodes::{LineColumn, Node, NodeAlert, NodeCode, NodeCodeBlock, NodeHtmlBlock, NodeValue},
};

use crate::{
    position::{Lines, split_lines},
    range, span,
};

/// What the formatter reads of the document it renders.
pub(crate) struct Document<'a> {
    /// The source, by line.
    pub lines: Lines<'a>,

    /// The `id` of the heading starting on each 0-based line that has one.
    pub anchors: &'a BTreeMap<usize, String>,
}

/// Format `node` on entering it, or finish it on leaving it.
pub(crate) fn format_node(
    context: &mut Context<'_, '_, &Document<'_>>,
    node: Node<'_>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    match node.data().value {
        NodeValue::Heading(ref heading) => heading_tag(context, node, heading.level, entering)?,
        NodeValue::Text(ref literal) => inline(context, node, literal, entering)?,
        NodeValue::HtmlInline(ref literal) => inline(context, node, literal, entering)?,
        NodeValue::Code(ref code) => code_span(context, node, code, entering)?,
        NodeValue::CodeBlock(ref block) => code_block(context, node, block, entering)?,
        NodeValue::HtmlBlock(ref block) => html_block(context, node, block, entering)?,
        NodeValue::Alert(ref alert) => alert_block(context, node, alert, entering)?,
        _ => return format_node_default(context, node, entering),
    }

    Ok(ChildRendering::HTML)
}

/// Write the opening tag of a heading of `level`, with the `id` the document's
/// anchors name for its line, or its closing tag.
fn heading_tag(
    context: &mut Context<'_, '_, &Document<'_>>,
    node: Node<'_>,
    level: u8,
    entering: bool,
) -> fmt::Result {
    if !entering {
        return writeln!(context, "</h{level}>");
    }

    let anchors = context.user.anchors;
    context.cr()?;
    write!(context, "<h{level}")?;
    // Positions count lines from 1.
    let line = node.data().sourcepos.start.line.saturating_sub(1);
    if let Some(id) = anchors.get(&line) {
        context.write_str(" id=\"")?;
        context.escape(id)?;
        context.write_str("\"")?;
    }
    context.write_str(">")
}

/// Write the literal of a text or raw HTML node.
fn inline(
    context: &mut Context<'_, '_, &Document<'_>>,
    node: Node<'_>,
    literal: &str,
    entering: bool,
) -> fmt::Result {
    if !entering {
        return Ok(());
    }

    let lines = &context.user.lines;
    let cell = cell_start(node);
    match range::inline(lines, node.data().sourcepos, cell, literal) {
        Some(range) => span::source(context, lines.text(), range, cell.is_some()),
        None => span::unmapped(context, literal),
    }
}

/// Write a code span, replacing each line ending in its content with an
/// unmapped space.
fn code_span(
    context: &mut Context<'_, '_, &Document<'_>>,
    node: Node<'_>,
    code: &NodeCode,
    entering: bool,
) -> fmt::Result {
    if !entering {
        return Ok(());
    }

    let lines = &context.user.lines;
    let cell = cell_start(node);
    context.write_str("<code>")?;
    match range::code_span(lines, node.data().sourcepos, cell, code) {
        Some(segments) => {
            for (index, segment) in segments.into_iter().enumerate() {
                if index > 0 {
                    context.write_str(" ")?;
                }
                span::source(context, lines.text(), segment, cell.is_some())?;
            }
        }
        None => span::unmapped(context, &code.literal)?,
    }
    context.write_str("</code>")
}

/// Write a code block, without syntax highlighting.
fn code_block(
    context: &mut Context<'_, '_, &Document<'_>>,
    node: Node<'_>,
    block: &NodeCodeBlock,
    entering: bool,
) -> fmt::Result {
    if !entering {
        return Ok(());
    }

    let lines = &context.user.lines;
    context.cr()?;
    context.write_str("<pre><code")?;
    if let Some(language) = block.info.split_ascii_whitespace().next() {
        context.write_str(" class=\"language-")?;
        context.escape(language)?;
        context.write_str("\"")?;
    }
    context.write_str(">")?;

    // A fenced block's content starts on the line after its opening fence.
    let first = node.data().sourcepos.start.line + usize::from(block.fenced);
    block_lines(context, lines, first, &block.literal)?;
    context.write_str("</code></pre>")?;
    context.lf()
}

/// Write a raw HTML block as escaped preformatted text.
fn html_block(
    context: &mut Context<'_, '_, &Document<'_>>,
    node: Node<'_>,
    block: &NodeHtmlBlock,
    entering: bool,
) -> fmt::Result {
    if !entering {
        return Ok(());
    }

    let lines = &context.user.lines;
    context.cr()?;
    context.write_str("<pre>")?;
    block_lines(
        context,
        lines,
        node.data().sourcepos.start.line,
        &block.literal,
    )?;
    context.write_str("</pre>")?;
    context.lf()
}

/// Write each line of `literal` as a span, starting at 1-based source line
/// `first`.
fn block_lines(
    out: &mut dyn fmt::Write,
    lines: &Lines<'_>,
    first: usize,
    literal: &str,
) -> fmt::Result {
    for (index, (content, ending)) in split_lines(literal).into_iter().enumerate() {
        match range::block_line(lines, first + index, content) {
            Some(range) => span::mapped(out, lines.text(), range)?,
            None => span::unmapped(out, content)?,
        }
        out.write_str(ending)?;
    }

    Ok(())
}

/// Write a GitHub alert block with its title wrapped in a span.
fn alert_block(
    context: &mut Context<'_, '_, &Document<'_>>,
    node: Node<'_>,
    alert: &NodeAlert,
    entering: bool,
) -> fmt::Result {
    if !entering {
        context.cr()?;
        context.write_str("</div>")?;
        return context.lf();
    }

    let lines = &context.user.lines;
    context.cr()?;
    write!(
        context,
        "<div class=\"markdown-alert {}\">",
        alert.alert_type.css_class()
    )?;
    context.lf()?;
    context.write_str("<p class=\"markdown-alert-title\">")?;
    alert_title(context, lines, node.data().sourcepos.start, alert)?;
    context.write_str("</p>")?;
    context.lf()
}

/// Write an alert's explicit title as a mapped span, or its default title as an
/// atom span covering the `[!KIND]` marker.
fn alert_title(
    out: &mut dyn fmt::Write,
    lines: &Lines<'_>,
    start: LineColumn,
    alert: &NodeAlert,
) -> fmt::Result {
    let Some(title) = &alert.title else {
        let title = alert.alert_type.default_title();
        return match range::alert_marker(lines, start, alert.alert_type) {
            Some(marker) => span::atom(out, marker, title),
            None => span::unmapped(out, title),
        };
    };

    match range::alert_title(lines, start, title) {
        Some(range) => span::mapped(out, lines.text(), range),
        None => span::unmapped(out, title),
    }
}

/// Return the start position of the enclosing table cell, if any.
fn cell_start(node: Node<'_>) -> Option<LineColumn> {
    node.ancestors().skip(1).find_map(|ancestor| {
        let data = ancestor.data();
        matches!(data.value, NodeValue::TableCell).then_some(data.sourcepos.start)
    })
}
