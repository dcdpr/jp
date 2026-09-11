//! Reference-definition preservation for terminal rendering.
//!
//! [`restore`] adds literal paragraphs for definitions consumed by comrak.

use std::ops::Range;

use comrak::{
    Arena, Node, Options,
    nodes::{Ast, NodeValue},
    parse_document,
};

/// Add literal paragraphs for reference definitions consumed during parsing.
///
/// `root` must be an unmodified comrak AST parsed from `source` with `options`.
/// Existing links keep their resolved destinations; definitions retain their
/// source order and container nesting.
pub fn restore<'a>(arena: &'a Arena<'a>, root: Node<'a>, source: &str, options: &Options<'_>) {
    if !source.contains("]:") {
        return;
    }

    // Source positions count CRLF and bare CR as single line endings.
    let source = source.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<_> = source.split_inclusive('\n').collect();
    // Only inspect the parser's nodes, not the literal paragraphs we insert.
    let nodes: Vec<_> = root.descendants().collect();
    for node in nodes {
        let data = node.data();
        let span = data.sourcepos;
        match data.value {
            NodeValue::Document
            | NodeValue::BlockQuote
            | NodeValue::Item(_)
            | NodeValue::TaskItem(_) => {
                drop(data);
                // Definition-only paragraphs leave gaps between child spans.
                let mut start = span.start.line.saturating_sub(1);
                let children: Vec<_> = node.children().collect();
                for child in children {
                    let child_span = child.data().sourcepos;
                    let end = child_span.start.line.saturating_sub(1);
                    let gap = container_source(node, &lines, start..end);
                    if definition_size(&gap, options) > 0 {
                        child.insert_before(literal_paragraph(arena, &gap));
                    }
                    start = child_span.end.line;
                }
                let gap = container_source(node, &lines, start..span.end.line);
                if definition_size(&gap, options) > 0 {
                    node.append(literal_paragraph(arena, &gap));
                }
            }
            NodeValue::Paragraph | NodeValue::Heading(_) => {
                drop(data);
                // The source span includes definitions removed before inline parsing.
                let text = container_source(
                    node,
                    &lines,
                    span.start.line.saturating_sub(1)..span.end.line,
                );
                let size = definition_size(&text, options);
                if size > 0 {
                    node.insert_before(literal_paragraph(arena, &text[..size]));
                }
            }
            _ => {}
        }
    }
}

/// Return the byte count of complete leading definitions, including whitespace.
fn definition_size(source: &str, options: &Options<'_>) -> usize {
    if !source.trim_start().starts_with('[') || !source.contains("]:") {
        return 0;
    }

    let leading = source.len() - source.trim_start().len();
    let mut size = 0;
    let mut end = 0;
    for line in source.split_inclusive('\n') {
        end += line.len();
        if end <= leading {
            continue;
        }
        let arena = Arena::new();
        let root = parse_document(&arena, &source[..end], options);
        if root.first_child().is_none() {
            size = end;
        }
        if line.trim().is_empty() && size > 0 {
            break;
        }
    }
    size
}

/// Build a paragraph from source text without parsing inline Markdown.
fn literal_paragraph<'a>(arena: &'a Arena<'a>, source: &str) -> Node<'a> {
    let paragraph = arena.alloc(Ast::new(NodeValue::Paragraph, (1, 1).into()).into());
    for line in source
        .trim_matches(['\n', '\r'])
        .trim_end()
        .split_inclusive('\n')
    {
        let content = line.trim_start_matches([' ', '\t']);
        let indent = &line[..line.len() - content.len()];
        // The terminal writer drops leading spaces in Text nodes when wrapping.
        // Raw nodes preserve indentation; the remaining text stays wrappable.
        if !indent.is_empty() {
            paragraph.append(
                arena.alloc(Ast::new(NodeValue::Raw(indent.to_owned()), (1, 1).into()).into()),
            );
        }
        paragraph.append(
            arena.alloc(Ast::new(NodeValue::Text(content.to_owned().into()), (1, 1).into()).into()),
        );
    }
    paragraph
}

/// Extract source lines without the list and blockquote prefixes of `node`.
fn container_source(node: Node<'_>, lines: &[&str], range: Range<usize>) -> String {
    let ancestors: Vec<_> = node.ancestors().collect();
    let mut source = String::new();
    for index in range {
        let Some(line) = lines.get(index) else {
            break;
        };
        let mut line = (*line).to_owned();
        let mut column = 0;
        for ancestor in ancestors.iter().rev() {
            let data = ancestor.data();
            match data.value {
                NodeValue::BlockQuote => {
                    let trimmed = line.trim_start_matches([' ', '\t']);
                    if trimmed.starts_with('>') {
                        let offset = line.len() - trimmed.len() + 1;
                        column = line[..offset].chars().fold(column, advance_column);
                        line.drain(..offset);
                        column += strip_columns(&mut line, 1, false, column);
                    }
                }
                NodeValue::Item(list) => {
                    column += strip_columns(
                        &mut line,
                        list.marker_offset + list.padding,
                        index + 1 == data.sourcepos.start.line,
                        column,
                    );
                }
                NodeValue::TaskItem(ref task) => {
                    if let Some(parent) = ancestor.parent()
                        && let NodeValue::List(list) = parent.data().value
                    {
                        // TaskItem has no padding field. Its checkbox position
                        // identifies the content column even when numbering
                        // crosses from 9 to 10.
                        let opening = lines[data.sourcepos.start.line - 1];
                        let marker = data.sourcepos.start.column - 1;
                        let checkbox = task.symbol_sourcepos.start.column - 1;
                        let start_column = opening[..marker].chars().fold(0, advance_column);
                        let end_column = opening[marker..checkbox]
                            .chars()
                            .fold(start_column, advance_column);
                        let count = list.marker_offset + end_column - start_column;
                        column += strip_columns(
                            &mut line,
                            count,
                            index + 1 == data.sourcepos.start.line,
                            column,
                        );
                    }
                }
                _ => {}
            }
        }
        source.push_str(&line);
    }
    source
}

/// Remove up to `count` source columns and return how many were consumed.
///
/// Tabs are measured from `start_column`; a partially consumed tab leaves
/// spaces.
/// `marker` permits stripping non-whitespace characters on an item's opening
/// line.
fn strip_columns(line: &mut String, count: usize, marker: bool, start_column: usize) -> usize {
    let mut column = start_column;
    let target = start_column + count;
    let mut offset = 0;
    for ch in line.chars() {
        if column >= target || ch == '\n' || (!marker && !matches!(ch, ' ' | '\t')) {
            break;
        }
        column = advance_column(column, ch);
        offset += ch.len_utf8();
    }
    line.replace_range(..offset, &" ".repeat(column.saturating_sub(target)));
    (column - start_column).min(count)
}

/// Advance a CommonMark source column, where tabs use four-column stops.
const fn advance_column(column: usize, ch: char) -> usize {
    column + if ch == '\t' { 4 - column % 4 } else { 1 }
}

#[cfg(test)]
#[path = "references_tests.rs"]
mod tests;
