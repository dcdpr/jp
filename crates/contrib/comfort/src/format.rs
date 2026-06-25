//! Pure source-string-in, source-string-out pipeline.
//!
//! The pipeline runs in three layers:
//!
//! 1. [`format_source`] finds `///` / `//!` doc-comment blocks via
//!    [`find_blocks`] and splices their reformatted bodies back into the source
//!    byte-for-byte.
//! 2. [`reflow_markdown`] parses each block's body with comrak, walks the AST
//!    recursively, and hands each reflowable paragraph's text to
//!    [`reflow_paragraph`].
//!    Leaf blocks that aren't paragraphs — reference link definitions, code
//!    blocks, headings, tables, HTML blocks, thematic breaks — are preserved
//!    verbatim, as are paragraphs that contain a hard line break (`  \n ` or
//!    `\\\n`).
//! 3. [`reflow_paragraph`] splits the paragraph into sentences with the
//!    [`sentence`] module and width-wraps each sentence via `textwrap`, keeping
//!    atomic tokens (URLs, paths, identifiers) intact even when they exceed
//!    `max_width`.
//!
//! Containers we descend into: [`BlockQuote`], [`List`], [`Item`],
//! [`TaskItem`], [`Alert`], [`MultilineBlockQuote`], [`FootnoteDefinition`],
//! [`BlockDirective`].
//! Each contributes a per-line continuation prefix that gets applied to every
//! line after the first (the first line's prefix is already in the source,
//! outside the Paragraph's sourcepos range).
//!
//! [`Alert`]: NodeValue::Alert
//! [`BlockDirective`]: NodeValue::BlockDirective
//! [`BlockQuote`]: NodeValue::BlockQuote
//! [`FootnoteDefinition`]: NodeValue::FootnoteDefinition
//! [`Item`]: NodeValue::Item
//! [`List`]: NodeValue::List
//! [`MultilineBlockQuote`]: NodeValue::MultilineBlockQuote
//! [`TaskItem`]: NodeValue::TaskItem
//! [`sentence`]: crate::sentence

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use comrak::{
    Arena, Options, ResolvedReference,
    nodes::{AstNode, NodeValue, TableAlignment},
    options::{BrokenLinkCallback, BrokenLinkReference, Extension, ListStyleType, Parse, Render},
};
use textwrap::WordSplitter;
use unicode_width::UnicodeWidthStr;

use crate::{extract::find_blocks, sentence::split_sentences};

/// Options that control which transformations the markdown pipeline applies on
/// top of the always-on sembr reflow.
/// Used by [`format_markdown_with`] and [`format_rust_source_with`].
#[derive(Debug, Clone, Default)]
pub struct FormatOptions {
    /// Maximum line width passed to the sembr engine.
    pub max_width: usize,
    /// `--format-markdown`: canonicalize markdown structure (tables, list
    /// markers, fences, etc.) via comrak's formatter plus our table aligner.
    pub canonical: bool,
    /// `--reference-links`: convert inline links to reference style and
    /// consolidate definitions at the bottom of each body.
    pub reference_links: bool,
    /// `--prune-reference-links`: remove reference definitions that no link or
    /// image resolves to.
    pub prune_reference_links: bool,
}

/// Reformat every `///` and `//!` block in `source`, returning the new text.
///
/// `max_width` is the maximum source-line width the user wants to enforce.
/// Per block, the effective width handed to the reflow step is reduced by the
/// block's prefix overhead — its leading indent plus the `///` or `//!` marker
/// and separator space — so the user-visible ceiling is honoured regardless of
/// how deep the doc comment is nested.
///
/// Returns the original `source` (as a fresh `String`) when no blocks need
/// reflow.
#[must_use]
pub fn format_source(source: &str, max_width: usize) -> String {
    format_rust_source_with(source, &FormatOptions {
        max_width,
        ..Default::default()
    })
}

/// Like [`format_source`], but also canonicalize the markdown inside each `///`
/// / `//!` block before reflowing it.
/// See [`format_markdown_canonical`] for what canonicalisation entails.
///
/// The doc-comment scaffolding (`///` prefix, indentation, surrounding code) is
/// still byte-preserved; only the *body* of each block is rewritten.
#[must_use]
pub fn format_source_canonical(source: &str, max_width: usize) -> String {
    format_rust_source_with(source, &FormatOptions {
        max_width,
        canonical: true,
        ..Default::default()
    })
}

/// Option-aware Rust-source entry point.
/// Each `///` / `//!` block's body goes through [`format_markdown_with`] with
/// the same options applied to it (with `max_width` adjusted for the block's
/// prefix overhead).
#[must_use]
pub fn format_rust_source_with(source: &str, opts: &FormatOptions) -> String {
    format_source_impl(source, opts.max_width, |body, effective_width| {
        let inner = FormatOptions {
            max_width: effective_width,
            ..opts.clone()
        };
        format_markdown_with(body, &inner)
    })
}

/// Option-aware markdown entry point.
/// Composes the optional canonical and reference-link passes before running the
/// always-on sembr reflow.
///
/// The output preserves the input's *exact* trailing-newline count.
/// This matters for callers that map newlines back to source structure — e.g.
/// nvim's conform.nvim `injected` formatter, which extracts the markdown body
/// of Rust doc comments, runs comfort on it, and re-inserts.
/// Collapsing `\n\n` to `\n` would silently drop the trailing empty `///` line
/// on every save.
#[must_use]
pub fn format_markdown_with(body: &str, opts: &FormatOptions) -> String {
    if body.is_empty() {
        return String::new();
    }
    let trailing_newlines = body
        .as_bytes()
        .iter()
        .rev()
        .take_while(|&&b| b == b'\n')
        .count();

    let mut text = if opts.canonical {
        // Comrak's `format_commonmark` unconditionally emits links in inline
        // form (`[text](url)`), dropping the user's reference definitions
        // along the way. Protect reference-form links by sentinelising them
        // and stashing definitions out-of-band before the canonical pass,
        // then restore both afterwards.
        let protection = protect_reference_form_links(body);
        let canonical = match canonicalize_markdown(&protection.protected_text) {
            Some(canonical) => align_tables(&canonical),
            None => protection.protected_text.clone(),
        };
        restore_protected_reference_links(&canonical, &protection)
    } else {
        body.to_owned()
    };
    if opts.prune_reference_links {
        text = prune_unused_reference_definitions(&text);
    }
    if opts.reference_links {
        text = extract_reference_links(&text);
    }
    let text = reflow_markdown(&text, opts.max_width);

    // Both `canonicalize_markdown` and `extract_reference_links` track
    // "trailing newline present?" but collapse multiple to one. Restore the
    // exact count from the input.
    let trimmed = text.trim_end_matches('\n');
    let mut out = String::with_capacity(trimmed.len() + trailing_newlines);
    out.push_str(trimmed);
    for _ in 0..trailing_newlines {
        out.push('\n');
    }
    out
}

/// Shared implementation for the `///`-block pipeline.
/// The body processor differs between default mode, `--format-markdown`, and
/// `--reference-links`; passed in by the caller.
fn format_source_impl<F>(source: &str, max_width: usize, process_body: F) -> String
where
    F: Fn(&str, usize) -> String,
{
    let blocks = find_blocks(source);
    if blocks.is_empty() {
        return source.to_owned();
    }

    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;

    for block in blocks {
        out.push_str(&source[cursor..block.range.start]);

        let body = block.lines.join("\n");
        // Subtract the per-line prefix from the user's budget. If the
        // prefix alone exceeds `max_width`, saturate to 0 (no width wrap)
        // rather than wrapping every word onto its own line — the user's
        // constraint is impossible here, so we degrade to pure sembr.
        let effective_width = if max_width == 0 {
            0
        } else {
            max_width.saturating_sub(block.prefix_width())
        };
        let formatted = process_body(&body, effective_width);
        out.push_str(&block.reassemble(&formatted));

        cursor = block.range.end;
    }

    out.push_str(&source[cursor..]);
    out
}

/// Canonicalize the markdown structure of `body` (align tables, normalise list
/// markers, prefer fenced code blocks, etc.) and then reflow its paragraphs
/// with semantic line breaks.
///
/// Canonicalisation is delegated to [`comrak::format_commonmark`] with our
/// render options (see `canonical_render_options` for the rationale); width
/// handling is left to our downstream sembr pipeline.
/// The output is the canonical markdown with paragraphs sembr'd.
///
/// The input's trailing-newline convention is preserved: doc-comment block
/// bodies (no trailing newline) round-trip without one; markdown files (usually
/// trailing newline) keep theirs.
#[must_use]
pub fn format_markdown_canonical(body: &str, max_width: usize) -> String {
    format_markdown_with(body, &FormatOptions {
        max_width,
        canonical: true,
        ..Default::default()
    })
}

/// Run comrak's `format_commonmark` over `body` and return the canonical
/// markdown text, with the input's trailing-newline convention preserved.
/// Returns `None` if the formatter errors — callers should fall back to the
/// input in that case.
fn canonicalize_markdown(body: &str) -> Option<String> {
    let arena = Arena::new();
    let parse_options = comrak_options();
    let root = comrak::parse_document(&arena, body, &parse_options);

    let render_options = canonical_render_options();
    let mut canonical = String::new();
    if comrak::format_commonmark(root, &render_options, &mut canonical).is_err() {
        return None;
    }

    // Comrak writes the active line prefix (`> ` for block quotes and alerts,
    // indentation for list items and footnotes) on blank lines too, leaving
    // trailing whitespace such as `> ` or `   `. Strip it before handing the
    // text downstream.
    let canonical = trim_blank_prefix_lines(&canonical);

    // Comrak's formatter appends a trailing newline unconditionally;
    // normalise to match the input's convention so the caller (block
    // reassembly for `///` blocks, file writes for `.md` files) sees a
    // consistent shape.
    let canonical = match (body.ends_with('\n'), canonical.ends_with('\n')) {
        (true, false) => canonical + "\n",
        (false, true) => canonical.trim_end_matches('\n').to_owned(),
        _ => canonical,
    };

    Some(canonical)
}

/// Trim trailing whitespace from blank lines that carry only block-quote (`>`)
/// markers or indentation, an artifact of comrak emitting the active line
/// prefix on blank lines.
///
/// Lines inside code blocks and HTML blocks are left alone: there a
/// `>`-and-spaces line is literal content (e.g. a markdown sample), not a
/// generated prefix.
fn trim_blank_prefix_lines(text: &str) -> String {
    if !text.split('\n').any(is_blank_prefix_line) {
        return text.to_owned();
    }

    let arena = Arena::new();
    let options = comrak_options();
    let root = comrak::parse_document(&arena, text, &options);
    let line_starts = line_start_offsets(text);

    let mut verbatim: Vec<Range<usize>> = Vec::new();
    collect_verbatim_block_ranges(root, text, &line_starts, &mut verbatim);

    let mut out = String::with_capacity(text.len());
    let mut byte_pos = 0_usize;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line_start = byte_pos;
        let line_end = byte_pos + line.len();
        byte_pos = line_end + 1;

        let in_verbatim = verbatim
            .iter()
            .any(|r| line_start >= r.start && line_end <= r.end);
        if in_verbatim || !is_blank_prefix_line(line) {
            out.push_str(line);
        } else {
            out.push_str(line.trim_end());
        }
    }
    out
}

/// A line that has trailing whitespace and contains nothing but block-quote
/// markers and whitespace, i.e. a blank line that comrak decorated with a line
/// prefix.
fn is_blank_prefix_line(line: &str) -> bool {
    line != line.trim_end() && line.chars().all(|c| c == '>' || c == ' ' || c == '\t')
}

/// Walk the AST for [`CodeBlock`] and [`HtmlBlock`] ranges, where line content
/// is literal and must not be trimmed.
///
/// [`CodeBlock`]: NodeValue::CodeBlock
/// [`HtmlBlock`]: NodeValue::HtmlBlock
fn collect_verbatim_block_ranges<'a>(
    node: &'a AstNode<'a>,
    text: &str,
    line_starts: &[usize],
    out: &mut Vec<Range<usize>>,
) {
    let data = node.data();
    if matches!(
        data.value,
        NodeValue::CodeBlock(_) | NodeValue::HtmlBlock(_)
    ) && let Some(range) = sourcepos_to_byte_range(line_starts, text.len(), &data.sourcepos)
    {
        out.push(range);
        return;
    }
    for child in node.children() {
        collect_verbatim_block_ranges(child, text, line_starts, out);
    }
}

/// Re-parse `text` to find markdown tables, then rewrite each one with column
/// widths padded for visual alignment.
/// The separator row's alignment markers come from the AST's [`TableAlignment`]
/// (the colon pattern in the source), not from re-scanning the text.
///
/// Tables are identified by [`NodeValue::Table`] nodes; cell content is taken
/// from each [`NodeValue::TableCell`]'s sourcepos slice, so any inline markdown
/// (`**bold**`, `` `code` ``) and escapes (`\|`) survive verbatim.
fn align_tables(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let arena = Arena::new();
    let options = comrak_options();
    let root = comrak::parse_document(&arena, text, &options);
    let line_starts = line_start_offsets(text);

    let mut replacements: Vec<Replacement> = Vec::new();
    collect_table_replacements(root, text, &line_starts, &mut replacements);

    if replacements.is_empty() {
        return text.to_owned();
    }

    replacements.sort_by_key(|r| r.range.start);

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for r in replacements {
        out.push_str(&text[cursor..r.range.start]);
        out.push_str(&r.text);
        cursor = r.range.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Walk the AST, queueing a [`Replacement`] for every table found.
fn collect_table_replacements<'a>(
    node: &'a AstNode<'a>,
    text: &str,
    line_starts: &[usize],
    out: &mut Vec<Replacement>,
) {
    let data = node.data();
    if let NodeValue::Table(table_meta) = &data.value {
        if let Some(range) = sourcepos_to_byte_range(line_starts, text.len(), &data.sourcepos)
            && let Some(aligned) =
                render_aligned_table(node, &table_meta.alignments, text, line_starts)
        {
            // Preserve the trailing newline convention of the source slice
            // — if the original ended with `\n`, the replacement should
            // too (and vice versa).
            let original_slice = &text[range.clone()];
            let aligned = match (original_slice.ends_with('\n'), aligned.ends_with('\n')) {
                (true, false) => aligned + "\n",
                (false, true) => aligned.trim_end_matches('\n').to_owned(),
                _ => aligned,
            };
            out.push(Replacement {
                range,
                text: aligned,
            });
        }
        // Don't descend further — tables don't nest within tables in our model.
        return;
    }
    for child in node.children() {
        collect_table_replacements(child, text, line_starts, out);
    }
}

/// Build the aligned markdown text for a single table node.
/// Returns `None` if the table is malformed (no rows, mismatched cell counts,
/// sourcepos gaps) — in which case the caller falls back to leaving the source
/// unchanged.
fn render_aligned_table<'a>(
    table: &'a AstNode<'a>,
    alignments: &[TableAlignment],
    text: &str,
    line_starts: &[usize],
) -> Option<String> {
    let num_cols = alignments.len();
    if num_cols == 0 {
        return None;
    }

    // Walk rows → cells, slicing each cell's source bytes via its sourcepos.
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row_node in table.children() {
        if !matches!(row_node.data().value, NodeValue::TableRow(_)) {
            continue;
        }
        let mut cells: Vec<String> = Vec::new();
        for cell_node in row_node.children() {
            if !matches!(cell_node.data().value, NodeValue::TableCell) {
                continue;
            }
            let cell_range =
                sourcepos_to_byte_range(line_starts, text.len(), &cell_node.data().sourcepos)?;
            // Trim the cell's source slice. Comrak's cell sourcepos usually
            // covers the content between the `|` delimiters with any leading
            // and trailing spaces, but trimming defensively handles both
            // shapes.
            let raw = text[cell_range].trim();
            cells.push(raw.to_owned());
        }
        rows.push(cells);
    }

    if rows.is_empty() {
        return None;
    }

    // Column widths: max display width per column, with a floor of 3 so
    // the separator row's alignment markers (`:-:`, `---`) always fit.
    // `UnicodeWidthStr::width` gives terminal-cell width — wide chars (CJK)
    // count as 2, zero-width chars as 0, which matches what a human eye
    // sees when scanning a column.
    let mut col_widths = vec![3_usize; num_cols];
    for row in &rows {
        for (col, cell) in row.iter().enumerate() {
            if col < num_cols {
                col_widths[col] = col_widths[col].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
    }

    // Emit. GFM tables: row 0 is the header; the separator row follows
    // (synthesised from `alignments`); remaining rows are data rows.
    let mut out = String::new();
    for (row_idx, row) in rows.iter().enumerate() {
        emit_data_row(&mut out, row, &col_widths, alignments, num_cols);
        if row_idx == 0 {
            emit_separator_row(&mut out, &col_widths, alignments, num_cols);
        }
    }

    Some(out)
}

fn emit_data_row(
    out: &mut String,
    row: &[String],
    col_widths: &[usize],
    alignments: &[TableAlignment],
    num_cols: usize,
) {
    out.push('|');
    for col in 0..num_cols {
        let cell = row.get(col).map_or("", String::as_str);
        let padded = pad_cell(cell, col_widths[col], alignments[col]);
        out.push(' ');
        out.push_str(&padded);
        out.push_str(" |");
    }
    out.push('\n');
}

fn emit_separator_row(
    out: &mut String,
    col_widths: &[usize],
    alignments: &[TableAlignment],
    num_cols: usize,
) {
    out.push('|');
    for col in 0..num_cols {
        let w = col_widths[col];
        let sep = match alignments[col] {
            // The colon-or-not pattern encodes alignment; width = `w`.
            TableAlignment::Left => format!(":{}", "-".repeat(w.saturating_sub(1))),
            TableAlignment::Right => format!("{}:", "-".repeat(w.saturating_sub(1))),
            TableAlignment::Center => format!(":{}:", "-".repeat(w.saturating_sub(2))),
            TableAlignment::None => "-".repeat(w),
        };
        out.push(' ');
        out.push_str(&sep);
        out.push_str(" |");
    }
    out.push('\n');
}

fn pad_cell(content: &str, width: usize, alignment: TableAlignment) -> String {
    let content_width = UnicodeWidthStr::width(content);
    let pad = width.saturating_sub(content_width);
    match alignment {
        TableAlignment::Right => format!("{}{content}", " ".repeat(pad)),
        TableAlignment::Center => {
            let left = pad / 2;
            let right = pad - left;
            format!("{}{content}{}", " ".repeat(left), " ".repeat(right))
        }
        TableAlignment::Left | TableAlignment::None => {
            format!("{content}{}", " ".repeat(pad))
        }
    }
}

// ---------------------------------------------------------------------------
// `--reference-links`: convert inline links to reference style and
// consolidate definitions at the bottom of the body.
// ---------------------------------------------------------------------------

/// Convert inline markdown links to reference-style links and move all
/// reference definitions to the bottom of `text`.
///
/// Adaptive label strategy:
///
/// - Shortcut form `[text]` when the link's text can serve as a unique label.
/// - Full form `[text][label]` when text collides with an already-used label
///   for a different URL (label gets a `-N` suffix).
///
/// Pre-existing scattered reference definitions are also moved to the bottom
/// and sorted alphabetically.
fn extract_reference_links(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let had_trailing_newline = text.ends_with('\n');

    // Pull out any existing `[label]: url "title"` definitions; the result
    // is the text minus those lines, plus a list of definitions.
    let (text_without_defs, existing_defs) = extract_existing_reference_definitions(text);

    // Seed the label map with existing definitions so newly converted
    // inline links can reuse them via full-form references.
    let mut label_map = LabelMap::default();
    let mut all_defs: Vec<LinkDef> = Vec::new();
    for def in existing_defs {
        label_map.register(&def);
        all_defs.push(def);
    }

    // Walk the AST for inline `Link` nodes and queue conversions. Each new
    // definition gets appended to `all_defs` as it's discovered.
    let arena = Arena::new();
    let options = comrak_options();
    let root = comrak::parse_document(&arena, &text_without_defs, &options);
    let line_starts = line_start_offsets(&text_without_defs);

    let mut replacements: Vec<Replacement> = Vec::new();
    collect_inline_link_replacements(
        root,
        &text_without_defs,
        &line_starts,
        &mut label_map,
        &mut all_defs,
        &mut replacements,
    );

    // Splice link replacements into the text.
    let text_after = if replacements.is_empty() {
        text_without_defs
    } else {
        replacements.sort_by_key(|r| r.range.start);
        let mut out = String::with_capacity(text_without_defs.len());
        let mut cursor = 0;
        for r in &replacements {
            out.push_str(&text_without_defs[cursor..r.range.start]);
            out.push_str(&r.text);
            cursor = r.range.end;
        }
        out.push_str(&text_without_defs[cursor..]);
        out
    };

    // Append all definitions, sorted alphabetically by label, at the
    // bottom of the body with a blank-line separator.
    let result = if all_defs.is_empty() {
        text_after
    } else {
        all_defs.sort_by(|a, b| a.label.cmp(&b.label));
        let mut result = text_after.trim_end().to_owned();
        if !result.is_empty() {
            result.push_str("\n\n");
        }
        for def in &all_defs {
            result.push_str(&def.render());
            result.push('\n');
        }
        // Strip the trailing `\n` we just added; the trailing-newline
        // adjustment below will put one back if the input had one.
        result.trim_end_matches('\n').to_owned()
    };

    if had_trailing_newline && !result.ends_with('\n') {
        format!("{result}\n")
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// `--prune-reference-links`: drop reference definitions that nothing cites.
// ---------------------------------------------------------------------------

/// Remove reference-link definitions in `text` that no link or image points at.
///
/// A definition is kept when some reference-form link or image (`[label]`,
/// `[label][]`, `[text][label]`) resolves to it.
/// Removal is in place: only the orphaned definition lines are spliced out and
/// every other byte is preserved.
/// A definition whose use comfort can't see (e.g. a cross-file reference) is
/// left alone — the pass never deletes on a guess.
fn prune_unused_reference_definitions(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let arena = Arena::new();
    let options = comrak_options();
    let root = comrak::parse_document(&arena, text, &options);
    let line_starts = line_start_offsets(text);

    let mut referenced: HashSet<String> = HashSet::new();
    collect_referenced_labels(root, text, &line_starts, text.len(), &mut referenced);

    let mut excluded: Vec<Range<usize>> = Vec::new();
    collect_excluded_ranges_for_refdefs(root, text, &line_starts, &mut excluded);

    let lines: Vec<&str> = text.split('\n').collect();
    let spans = collect_reference_definition_spans(&lines, &line_starts, text.len(), &excluded);

    let mut removals: Vec<Range<usize>> = spans
        .iter()
        .filter(|s| !referenced.contains(&normalize_label(&s.def.label)))
        .map(|s| s.byte_range.clone())
        .collect();
    if removals.is_empty() {
        return text.to_owned();
    }
    removals.sort_by_key(|r| r.start);

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for r in removals {
        out.push_str(&text[cursor..r.start]);
        cursor = r.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Walk the AST collecting the normalized labels of every reference-form link
/// and image that resolved to a definition.
/// Inline links/images (`[text](url)`) and autolinks reference no definition
/// and contribute nothing.
fn collect_referenced_labels<'a>(
    node: &'a AstNode<'a>,
    text: &str,
    line_starts: &[usize],
    body_len: usize,
    out: &mut HashSet<String>,
) {
    let data = node.data();
    if matches!(data.value, NodeValue::Link(_) | NodeValue::Image(_)) {
        if let Some(range) = sourcepos_to_byte_range(line_starts, body_len, &data.sourcepos) {
            // Images carry a leading `!`; the bracket structure after it is
            // identical to a link's.
            let slice_full = &text[range];
            let slice = slice_full.strip_prefix('!').unwrap_or(slice_full);
            if is_reference_form_link(slice)
                && let Some(label) = reference_link_label(slice)
            {
                out.insert(normalize_label(&label));
            }
        }
        return;
    }
    for child in node.children() {
        collect_referenced_labels(child, text, line_starts, body_len, out);
    }
}

/// Extract the reference label a reference-form link or image points at.
/// `[text][label]` → `label`; collapsed `[label][]` and shortcut `[label]` →
/// `label`.
/// Returns `None` when `slice` isn't bracket-formed.
fn reference_link_label(slice: &str) -> Option<String> {
    let bytes = slice.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let first_close = match_close_bracket(bytes, 0)?;
    if bytes.get(first_close + 1) == Some(&b'[')
        && let Some(second_close) = match_close_bracket(bytes, first_close + 1)
    {
        let second = &slice[first_close + 2..second_close];
        if !second.trim().is_empty() {
            return Some(second.to_owned());
        }
    }
    Some(slice[1..first_close].to_owned())
}

/// A single CommonMark reference-link definition.
/// `title` is empty when the definition has no title; otherwise it's the
/// unescaped title text (matching how comrak hands us inline-link titles).
#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkDef {
    label: String,
    url: String,
    title: String,
}

impl LinkDef {
    /// Render as a definition line, without the trailing newline.
    /// Uses double-quoted title syntax; embedded backslashes and double quotes
    /// are backslash-escaped (CommonMark allows backslash escapes in titles).
    fn render(&self) -> String {
        if self.title.is_empty() {
            format!("[{}]: {}", self.label, self.url)
        } else {
            let escaped = self.title.replace('\\', r"\\").replace('"', r#"\""#);
            format!("[{}]: {} \"{escaped}\"", self.label, self.url)
        }
    }
}

/// Normalize a reference label per CommonMark §4.7: Unicode case fold, then
/// trim outer whitespace and collapse internal whitespace runs to a single
/// space.
/// Two labels match when their normalized forms are equal.
///
/// We use `str::to_lowercase` as a pragmatic stand-in for full Unicode case
/// folding — it covers ASCII and the Latin/Cyrillic/Greek scripts that show up
/// in practice, without pulling in a new dependency.
fn normalize_label(label: &str) -> String {
    label
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Label registry that tracks bidirectional label ↔ (url, title) mapping.
/// Used to decide whether a new inline link can reuse an existing definition
/// (shortcut form, full form, or a fresh definition).
///
/// `by_label` is keyed by the *normalized* label (CommonMark §4.7 —
/// case-insensitive, whitespace-folded), so an existing `[Foo]: /old` collides
/// with an inline `[foo](/new)` as the renderer would: without that, we'd emit
/// two definitions sharing one canonical label and the renderer would resolve
/// the converted shortcut to whichever came first.
///
/// `by_url_title` keys on the literal `(url, title)` tuple so two links
/// pointing at the same URL with different titles get distinct definitions —
/// otherwise the title metadata of one would be silently dropped.
/// Its values are the *original-cased* labels, so full-form references write
/// `[text][Foo]` (the casing the definition is stored under) rather than the
/// normalized form.
#[derive(Debug, Default)]
struct LabelMap {
    by_label: HashMap<String, (String, String)>,
    by_url_title: HashMap<(String, String), String>,
}

impl LabelMap {
    /// Register a definition.
    /// If the `(url, title)` pair doesn't already have a canonical label, this
    /// one becomes it.
    fn register(&mut self, def: &LinkDef) {
        self.by_label
            .entry(normalize_label(&def.label))
            .or_insert_with(|| (def.url.clone(), def.title.clone()));
        self.by_url_title
            .entry((def.url.clone(), def.title.clone()))
            .or_insert_with(|| def.label.clone());
    }

    /// Resolve an inline `[text](url "title")` link to its reference-form
    /// replacement and, if a new definition was needed, append it to `defs`.
    fn resolve_inline(
        &mut self,
        text: &str,
        url: &str,
        title: &str,
        defs: &mut Vec<LinkDef>,
    ) -> String {
        // (url, title) already has a canonical label?
        let key = (url.to_owned(), title.to_owned());
        if let Some(existing_label) = self.by_url_title.get(&key) {
            let existing_label = existing_label.clone();
            return if existing_label == text {
                format!("[{text}]")
            } else {
                format!("[{text}][{existing_label}]")
            };
        }
        // New (url, title) — pick a label. Use the link text if its
        // normalized form is free; otherwise disambiguate with a numeric
        // suffix. Collision checks go through `normalize_label` so we don't
        // emit `[foo]: /new` next to an existing `[Foo]: /old`.
        let label = if self.by_label.contains_key(&normalize_label(text)) {
            let mut i = 2_usize;
            loop {
                let candidate = format!("{text}-{i}");
                if !self.by_label.contains_key(&normalize_label(&candidate)) {
                    break candidate;
                }
                i += 1;
            }
        } else {
            text.to_owned()
        };
        self.by_label
            .insert(normalize_label(&label), (url.to_owned(), title.to_owned()));
        self.by_url_title.insert(key, label.clone());
        defs.push(LinkDef {
            label: label.clone(),
            url: url.to_owned(),
            title: title.to_owned(),
        });
        if label == text {
            format!("[{text}]")
        } else {
            format!("[{text}][{label}]")
        }
    }
}

/// Walk the AST for inline `Link` nodes.
/// For each, queue a [`Replacement`] of its source bytes with the
/// reference-form output.
/// Anchor links (`#fragment`), images, autolinks, and pre-existing
/// reference-form links are left alone.
fn collect_inline_link_replacements<'a>(
    node: &'a AstNode<'a>,
    text: &str,
    line_starts: &[usize],
    label_map: &mut LabelMap,
    defs: &mut Vec<LinkDef>,
    out: &mut Vec<Replacement>,
) {
    let data = node.data();
    match &data.value {
        NodeValue::Link(link) => {
            // Skip anchor-only URLs (`#foo`) and images (Image is its own
            // NodeValue variant so the match below handles that).
            if !link.url.starts_with('#')
                && let Some(range) =
                    sourcepos_to_byte_range(line_starts, text.len(), &data.sourcepos)
                && let Some(link_text) = parse_inline_link_text(&text[range.clone()])
            {
                let replacement =
                    label_map.resolve_inline(&link_text, &link.url, &link.title, defs);
                out.push(Replacement {
                    range,
                    text: replacement,
                });
            }
            // Don't descend into Link children — they're inlines that get
            // included in the replacement text already.
            return;
        }
        NodeValue::Image(_) => {
            // Per design: leave images as inline `![alt](url)`. Don't recurse.
            return;
        }
        _ => {}
    }
    for child in node.children() {
        collect_inline_link_replacements(child, text, line_starts, label_map, defs, out);
    }
}

/// If `slice` is the source of an inline-form link `[text](url)`, return the
/// raw text between `[` and `](`.
/// Returns `None` for reference-form links (`[text][label]`, `[label][]`,
/// `[label]`) and for autolinks.
fn parse_inline_link_text(slice: &str) -> Option<String> {
    let bytes = slice.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let mut depth = 0_i32;
    let mut i = 0_usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                // Backslash-escape: skip the next byte too.
                i += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    // Inline form requires `(` immediately after the
                    // matched `]`. Anything else (`[`, end-of-slice,
                    // whitespace) is a reference form or invalid.
                    if bytes.get(i + 1) == Some(&b'(') {
                        // Text is between the opening `[` (index 0) and the
                        // closing `]` (index i).
                        return Some(slice[1..i].to_owned());
                    }
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Find pre-existing reference definitions in `text`, returning the text with
/// those definitions removed and the list of parsed definitions.
///
/// Definitions may span multiple lines (a destination or title wrapped onto a
/// following line); each is recognised as a whole via
/// [`collect_reference_definition_spans`].
/// Lines inside fenced code blocks, HTML blocks, and paragraphs are skipped,
/// identified via comrak's AST so we don't false-match content that merely
/// looks like a definition.
fn extract_existing_reference_definitions(text: &str) -> (String, Vec<LinkDef>) {
    let arena = Arena::new();
    let options = comrak_options();
    let root = comrak::parse_document(&arena, text, &options);
    let line_starts = line_start_offsets(text);

    let mut excluded: Vec<Range<usize>> = Vec::new();
    collect_excluded_ranges_for_refdefs(root, text, &line_starts, &mut excluded);

    let lines: Vec<&str> = text.split('\n').collect();
    let spans = collect_reference_definition_spans(&lines, &line_starts, text.len(), &excluded);

    let mut removed = vec![false; lines.len()];
    let mut defs: Vec<LinkDef> = Vec::with_capacity(spans.len());
    for span in spans {
        for k in span.line_range.clone() {
            removed[k] = true;
        }
        defs.push(span.def);
    }

    let content_lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter_map(|(k, line)| (!removed[k]).then_some(*line))
        .collect();

    (content_lines.join("\n"), defs)
}

/// A reference definition occupying one or more consecutive source lines.
struct RefDefSpan {
    /// Half-open range of line indices the definition occupies.
    line_range: Range<usize>,
    /// Byte range covering those lines, including the trailing newline of the
    /// last line when present, so splicing it out leaves no blank line behind.
    byte_range: Range<usize>,
    /// The verbatim source text of the span (newlines preserved).
    text: String,
    /// The parsed definition.
    def: LinkDef,
}

/// Group a line stream into reference-definition spans.
///
/// A span begins at a line [`looks_like_definition_start`] accepts and extends
/// across the following lines until a blank line, an excluded line
/// (code/HTML/paragraph), the next definition start, or end of input — i.e.
/// the continuation lines carrying a destination or title that wrapped onto
/// their own line.
/// The whole span is parsed as one unit, so a definition whose URL or title
/// sits on a later line is recognised correctly.
///
/// Spans whose joined text fails to parse as a definition are dropped.
fn collect_reference_definition_spans(
    lines: &[&str],
    line_starts: &[usize],
    text_len: usize,
    excluded: &[Range<usize>],
) -> Vec<RefDefSpan> {
    let is_excluded = |i: usize| {
        let start = line_starts[i];
        let end = start + lines[i].len();
        excluded.iter().any(|r| start >= r.start && end <= r.end)
    };

    let mut spans = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_excluded(i) || !looks_like_definition_start(lines[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        let mut j = i + 1;
        while j < lines.len()
            && !is_excluded(j)
            && !lines[j].trim().is_empty()
            && !looks_like_definition_start(lines[j])
        {
            end = j;
            j += 1;
        }
        let text = lines[start..=end].join("\n");
        if let Some(def) = parse_reference_definition(&text) {
            let byte_start = line_starts[start];
            let last_end = line_starts[end] + lines[end].len();
            let byte_end = if last_end < text_len {
                last_end + 1
            } else {
                last_end
            };
            spans.push(RefDefSpan {
                line_range: start..end + 1,
                byte_range: byte_start..byte_end,
                text,
                def,
            });
            i = end + 1;
        } else {
            i += 1;
        }
    }
    spans
}

/// Walk the AST for block ranges where a `[label]: url` shape must NOT be
/// extracted as a reference definition.
///
/// - [`CodeBlock`] / [`HtmlBlock`]: the bracket pattern is literal content.
/// - [`Paragraph`]: CommonMark forbids reference definitions from interrupting
///   a paragraph, so a `[label]: url` line that comrak parsed as part of a
///   paragraph's sourcepos is visible prose, not a definition.
///
/// [`CodeBlock`]: NodeValue::CodeBlock
/// [`HtmlBlock`]: NodeValue::HtmlBlock
/// [`Paragraph`]: NodeValue::Paragraph
fn collect_excluded_ranges_for_refdefs<'a>(
    node: &'a AstNode<'a>,
    text: &str,
    line_starts: &[usize],
    out: &mut Vec<Range<usize>>,
) {
    let data = node.data();
    if matches!(
        data.value,
        NodeValue::CodeBlock(_) | NodeValue::HtmlBlock(_) | NodeValue::Paragraph
    ) && let Some(range) = sourcepos_to_byte_range(line_starts, text.len(), &data.sourcepos)
    {
        out.push(range);
        // Paragraphs have only inline children; code and HTML blocks are
        // leaves. No further recursion needed.
        return;
    }
    for child in node.children() {
        collect_excluded_ranges_for_refdefs(child, text, line_starts, out);
    }
}

/// Parse a reference definition `[label]: url "title"` from `span`.
///
/// `span` may be one line or several joined with newlines: CommonMark allows a
/// line ending at each whitespace separator between label, destination, and
/// title.
/// The newlines are folded to spaces before tokenizing, so a URL or title that
/// wrapped onto a later line parses the same as the single-line form.
/// A multi-line title's internal break collapses to a single space.
///
/// Title is optional and may be enclosed in `"..."`, `'...'`, or `(...)`.
/// Backslash escapes inside the title are unescaped (CommonMark semantics) so
/// the stored value matches how comrak gives us inline-link titles.
/// Returns `None` when `span` doesn't match the reference-definition shape.
fn parse_reference_definition(span: &str) -> Option<LinkDef> {
    let line = span.replace('\n', " ");
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    // CommonMark allows up to 3 spaces of indentation.
    if indent > 3 || !trimmed.starts_with('[') {
        return None;
    }

    // Find the matching `]`, allowing nested `[...]` inside the label.
    let bytes = trimmed.as_bytes();
    let mut depth = 0_i32;
    let mut close = None;
    let mut i = 0_usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let close = close?;
    let label = &trimmed[1..close];
    if label.is_empty() {
        return None;
    }
    // Footnote definitions (`[^label]: ...`) are handled by the
    // footnotes extension, not as regular reference definitions. If we
    // extracted them here, the protection round-trip would strip them
    // before the canonical pass and re-emit them at the bottom — by
    // which time comrak has parsed `[^label]` in prose as an undefined
    // reference and escaped it as `[^label]`.
    if label.starts_with('^') {
        return None;
    }

    let after = &trimmed[close + 1..];
    let after = after.strip_prefix(':')?.trim_start();
    if after.is_empty() {
        return None;
    }

    // Split URL from optional title. The URL is either `<...>` or the first
    // run of non-whitespace bytes; the title (if any) follows after
    // whitespace.
    let (url, rest) = if let Some(after_lt) = after.strip_prefix('<') {
        let end = after_lt.find('>')?;
        (after_lt[..end].to_owned(), &after_lt[end + 1..])
    } else {
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        (after[..end].to_owned(), &after[end..])
    };
    if url.is_empty() {
        return None;
    }

    let rest = rest.trim();
    let title = if rest.is_empty() {
        String::new()
    } else {
        // If the trailing text isn't a well-formed title, treat the line as
        // not a reference definition at all — trailing junk would otherwise
        // round-trip lossily.
        parse_quoted_title(rest)?
    };

    Some(LinkDef {
        label: label.to_owned(),
        url,
        title,
    })
}

/// True if `line` begins a reference definition: up to three spaces of indent,
/// then `[label]:`.
/// Used to segment the line stream into spans before handing each to
/// [`parse_reference_definition`].
/// Footnote definitions (`[^name]:`) are excluded — the footnotes extension
/// handles those.
fn looks_like_definition_start(line: &str) -> bool {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if indent > 3 || !trimmed.starts_with('[') {
        return false;
    }
    let Some(close) = match_close_bracket(trimmed.as_bytes(), 0) else {
        return false;
    };
    let label = &trimmed[1..close];
    if label.is_empty() || label.starts_with('^') {
        return false;
    }
    trimmed[close + 1..].starts_with(':')
}

/// Byte index of the `]` matching the `[` at `open`, honoring nested brackets
/// and backslash escapes.
/// `None` if the bracket is unbalanced.
fn match_close_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0_i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse a CommonMark reference-definition title.
/// Accepts the three CommonMark forms: `"..."`, `'...'`, or `(...)`.
/// Backslash escapes inside the title are unescaped.
fn parse_quoted_title(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let (open, close) = match bytes.first()? {
        b'"' => (b'"', b'"'),
        b'\'' => (b'\'', b'\''),
        b'(' => (b'(', b')'),
        _ => return None,
    };
    // The closing delimiter must be the last byte. `"..."trailing` is not
    // a well-formed title.
    if bytes.len() < 2 || bytes[bytes.len() - 1] != close {
        return None;
    }
    let inner = &s[1..s.len() - 1];
    // Reject unbalanced delimiters of the same kind inside the body — e.g.
    // `"foo"bar"` would otherwise parse as `foo"bar`. For parens we don't
    // try to balance properly; nested unescaped parens are rare in titles.
    let mut unescaped = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                unescaped.push(next);
            }
            continue;
        }
        if c as u32 == u32::from(open) && open == close {
            return None;
        }
        unescaped.push(c);
    }
    Some(unescaped)
}

// ---------------------------------------------------------------------------
// Reference-form link protection across the canonical pass.
//
// Comrak's `format_commonmark` always emits links inline (`[text](url)`)
// regardless of how they appeared in the source. It also drops orphaned
// reference definitions once all references have been inlined. To preserve
// the user's choice of reference form (and their label names), we wrap the
// canonical pass with two helpers:
//
// 1. `protect_reference_form_links`: substitute citations with alphanumeric
//    sentinels and stash definitions out-of-band.
// 2. `restore_protected_reference_links`: replace sentinels with original
//    citation bytes and re-append definitions at the end of the body.
//
// The sentinels are bare alphanumeric strings, which comrak treats as plain
// text and emits verbatim through its parse + serialize cycle.
// ---------------------------------------------------------------------------

struct LinkProtection {
    /// Sentinel-substituted text fed to the canonical pass.
    protected_text: String,
    /// For each citation: (sentinel string, original source bytes).
    citations: Vec<(String, String)>,
    /// Original reference-definition lines, in source order, to re-append after
    /// canonical.
    /// The text-without-defs is what we sentinelise and pass to the canonical
    /// pass.
    definitions: Vec<String>,
}

fn protect_reference_form_links(text: &str) -> LinkProtection {
    if text.is_empty() {
        return LinkProtection {
            protected_text: String::new(),
            citations: Vec::new(),
            definitions: Vec::new(),
        };
    }

    let arena = Arena::new();
    let options = comrak_options_with_intra_doc_links();
    let root = comrak::parse_document(&arena, text, &options);
    let line_starts = line_start_offsets(text);

    // Collect citation source ranges (reference-form links only — inline
    // links and autolinks are left alone).
    let mut citation_ranges: Vec<Range<usize>> = Vec::new();
    collect_reference_form_link_ranges(root, text, &line_starts, &mut citation_ranges);

    // Collect reference definition line ranges, excluding code blocks and
    // HTML blocks (where `[label]: url` patterns are content, not defs).
    let mut excluded: Vec<Range<usize>> = Vec::new();
    collect_excluded_ranges_for_refdefs(root, text, &line_starts, &mut excluded);

    let lines: Vec<&str> = text.split('\n').collect();
    let spans = collect_reference_definition_spans(&lines, &line_starts, text.len(), &excluded);
    let mut definitions: Vec<String> = Vec::with_capacity(spans.len());
    let mut definition_ranges: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        definition_ranges.push(span.byte_range);
        definitions.push(span.text);
    }

    // Build the sentinel-substituted text.
    let mut substitutions: Vec<(Range<usize>, String)> = Vec::new();
    let mut citations: Vec<(String, String)> = Vec::new();
    for range in citation_ranges {
        let sentinel = format!("XCMFRTLR{:04}X", citations.len());
        let original = text[range.clone()].to_owned();
        substitutions.push((range, sentinel.clone()));
        citations.push((sentinel, original));
    }
    for range in definition_ranges {
        substitutions.push((range, String::new()));
    }
    substitutions.sort_by_key(|(r, _)| r.start);

    let mut protected_text = String::with_capacity(text.len());
    let mut cursor = 0_usize;
    for (range, replacement) in substitutions {
        protected_text.push_str(&text[cursor..range.start]);
        protected_text.push_str(&replacement);
        cursor = range.end;
    }
    protected_text.push_str(&text[cursor..]);

    LinkProtection {
        protected_text,
        citations,
        definitions,
    }
}

fn restore_protected_reference_links(canonical: &str, protection: &LinkProtection) -> String {
    let had_trailing_newline = canonical.ends_with('\n');
    let mut text = canonical.to_owned();

    // Step 1: replace sentinels with original citation source.
    for (sentinel, original) in &protection.citations {
        text = text.replace(sentinel, original);
    }

    // Step 2: re-append definitions at the bottom (separated by a blank
    // line). If `--reference-links` is also enabled, the subsequent
    // `extract_reference_links` pass will re-sort and consolidate.
    if !protection.definitions.is_empty() {
        let trimmed = text.trim_end();
        let mut result = trimmed.to_owned();
        if !result.is_empty() {
            result.push_str("\n\n");
        }
        for def in &protection.definitions {
            result.push_str(def);
            result.push('\n');
        }
        text = result.trim_end_matches('\n').to_owned();
    }

    if had_trailing_newline && !text.ends_with('\n') {
        text.push('\n');
    } else if !had_trailing_newline {
        text = text.trim_end_matches('\n').to_owned();
    }

    text
}

/// Walk the AST for [`NodeValue::Link`] nodes whose source slice is
/// reference-form (`[text][label]`, `[label][]`, or shortcut `[label]`).
/// Skips inline links, autolinks, and images.
fn collect_reference_form_link_ranges<'a>(
    node: &'a AstNode<'a>,
    text: &str,
    line_starts: &[usize],
    out: &mut Vec<Range<usize>>,
) {
    let data = node.data();
    match &data.value {
        NodeValue::Link(_) => {
            if let Some(range) = sourcepos_to_byte_range(line_starts, text.len(), &data.sourcepos)
                && is_reference_form_link(&text[range.clone()])
            {
                out.push(range);
            }
            return;
        }
        NodeValue::Image(_) => {
            // Don't recurse into images. Reference-form images would also be
            // inlined by comrak, but extending protection to them is a
            // separate concern — the present bug is link-only.
            return;
        }
        _ => {}
    }
    for child in node.children() {
        collect_reference_form_link_ranges(child, text, line_starts, out);
    }
}

/// Returns `true` when the source slice is the source of a reference-form link.
/// Inline links (slice ends with `](url)`) and autolinks (slice starts with
/// `<`) return `false`.
fn is_reference_form_link(slice: &str) -> bool {
    let bytes = slice.as_bytes();
    if bytes.first() != Some(&b'[') {
        // Autolink `<url>` or some other non-bracket-prefixed link.
        return false;
    }
    let mut depth = 0_i32;
    let mut i = 0_usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    // Inline form would have `(` immediately after the
                    // matched `]`. Anything else (`[`, EOL, whitespace) is
                    // reference form.
                    return bytes.get(i + 1) != Some(&b'(');
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Render options for canonical-markdown output: comrak's defaults with our
/// tweaks.
///
/// `width = usize::MAX` is deliberate.
/// Counter-intuitively, `width = 0` makes comrak's formatter *preserve source
/// soft breaks within paragraphs*, which leaves digit-period sequences (`404.`)
/// and other otherwise-meaningful characters at the start of continuation
/// lines. comrak then defensively escapes them (`404\.`) so that re-parsing the
/// canonical output produces the same AST.
/// The escapes are visible to the user as cosmetic noise.
///
/// Setting `width = usize::MAX` makes comrak collapse soft breaks: each
/// paragraph emits as one logical line, putting those characters mid-line where
/// no escape is needed.
/// Our downstream sembr pass then handles width-wrapping, so the lost soft
/// breaks are immediately replaced with sentence-per-line layout.
///
/// The other choices match `jp_md`'s existing conventions.
fn canonical_render_options() -> Options<'static> {
    let mut options = comrak_options();
    options.render = Render {
        width: usize::MAX,
        list_style: ListStyleType::Dash,
        prefer_fenced: true,
        ..Default::default()
    };
    options
}

/// Replace every top-level paragraph in a markdown body with its reflowed
/// version.
/// Other block types are left as-is.
#[must_use]
pub fn reflow_markdown(body: &str, max_width: usize) -> String {
    if body.is_empty() {
        return String::new();
    }

    let arena = Arena::new();
    let options = comrak_options();
    let root = comrak::parse_document(&arena, body, &options);

    let line_starts = line_start_offsets(body);
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut ancestors: Vec<&AstNode<'_>> = Vec::new();
    collect_paragraphs(
        root,
        &mut ancestors,
        &mut replacements,
        body,
        &line_starts,
        max_width,
    );

    if replacements.is_empty() {
        return body.to_owned();
    }

    // Comrak doesn't guarantee AST order matches source order: footnote
    // definitions in particular get reordered (the definition appears in
    // the AST after the paragraph that references it, regardless of where
    // it lived in the source). Sort by source byte offset before splicing
    // so the cursor walks the body in monotonic order.
    replacements.sort_by_key(|r| r.range.start);

    let mut out = String::with_capacity(body.len());
    let mut cursor = 0;
    for r in replacements {
        out.push_str(&body[cursor..r.range.start]);
        out.push_str(&r.text);
        cursor = r.range.end;
    }
    out.push_str(&body[cursor..]);
    out
}

/// Resolver that turns unresolved shortcut/collapsed references (`[X]` or
/// `[X][]`) into dummy `Link` AST nodes — specifically the ones that look like
/// Rust intra-doc links (`[`foo`]`, `[crate::Foo]`, etc.).
/// Without this, comrak's parser treats unresolved references as plain text
/// with literal `[` and `]`, which the formatter then defensively escapes as
/// `[X]`.
/// By forcing intra-doc-like labels to be `Link` nodes,
/// [`protect_reference_form_links`] can sentinelise their source bytes and
/// bypass comrak's escape logic entirely.
///
/// **Critically narrow.** The callback must *not* match task-list markers (`[
/// ]`, `[x]`, `[X]`) or footnote references (`[^name]`): `broken_link_callback`
/// fires before the `tasklist` / `footnotes` extensions get to recognise them,
/// so a too-eager callback eats task items and footnotes silently.
/// Returning `None` for those patterns lets the extensions handle them.
///
/// The dummy URL is empty; the value never reaches output because protection
/// substitutes the source bytes back verbatim.
struct ResolveIntraDocLinks;

impl BrokenLinkCallback for ResolveIntraDocLinks {
    fn resolve(&self, link: BrokenLinkReference<'_>) -> Option<ResolvedReference> {
        let label = link.normalized.trim();
        // Footnote references: handled by the footnotes extension.
        if label.starts_with('^') {
            return None;
        }
        // Task-list markers: `[ ]` normalises to empty, `[x]` / `[X]`
        // normalise to single characters. Let the tasklist extension
        // recognise them.
        if label.is_empty() || label.eq_ignore_ascii_case("x") {
            return None;
        }
        Some(ResolvedReference {
            url: String::new(),
            title: String::new(),
        })
    }
}

/// Build the comrak parse options used throughout the pipeline.
/// Factored out so the re-parse for block-quote-nested paragraphs (see
/// [`collect_inline_atomic_ranges_from_text`]) uses the exact same extension
/// set.
///
/// Note: this is the *plain* parse options without the intra-doc broken-link
/// callback.
/// The callback would interfere with the tasklist and footnotes extensions (see
/// [`ResolveIntraDocLinks`]).
/// Use [`comrak_options_with_intra_doc_links`] only where the callback's effect
/// is genuinely needed — currently only [`protect_reference_form_links`].
fn comrak_options() -> Options<'static> {
    Options {
        extension: Extension {
            table: true,
            tasklist: true,
            alerts: true,
            multiline_block_quotes: true,
            footnotes: true,
            block_directive: true,
            // Detect YAML frontmatter (`---` at the top of a document).
            // Required for markdown files; benign for doc comments because
            // frontmatter only triggers when the first non-empty line of
            // the document is the delimiter, which is almost never the case
            // inside a `///` block.
            front_matter_delimiter: Some("---".to_owned()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Parse options with the [`ResolveIntraDocLinks`] callback enabled, so
/// unresolved intra-doc shortcut/collapsed references become `Link` nodes in
/// the AST.
/// Used exclusively by [`protect_reference_form_links`] to find these
/// references and sentinelise their source bytes.
fn comrak_options_with_intra_doc_links() -> Options<'static> {
    let mut options = comrak_options();
    options.parse = Parse {
        broken_link_callback: Some(Arc::new(ResolveIntraDocLinks)),
        ..Default::default()
    };
    options
}

/// Recursively walk the AST collecting paragraphs to reflow.
///
/// Descends into the container types matched explicitly below.
/// Other containers (e.g.
/// `DescriptionList`) and leaf blocks (code blocks, headings, tables, HTML
/// blocks) are skipped, so their content survives verbatim.
///
/// Paragraphs that contain a [`LineBreak`] inline child — i.e. an explicit
/// markdown hard break — are also left untouched.
/// `collapse_whitespace` in the sembr step would otherwise silently eat the
/// hard-break marker, changing how rustdoc renders the paragraph.
/// The same coarse-grained rule we apply to code blocks and tables: when reflow
/// would lose information, opt out of reflow for the whole element.
///
/// [`LineBreak`]: NodeValue::LineBreak
fn collect_paragraphs<'a>(
    node: &'a AstNode<'a>,
    ancestors: &mut Vec<&'a AstNode<'a>>,
    out: &mut Vec<Replacement>,
    body: &str,
    line_starts: &[usize],
    max_width: usize,
) {
    let data = node.data();
    match &data.value {
        NodeValue::Paragraph => {
            // Hard breaks (`  \n` or `\\\n`) mean the user deliberately
            // chose where lines break; reflowing would silently destroy
            // that intent. Leave the paragraph verbatim and skip ahead.
            if has_hard_line_break(node) {
                return;
            }
            let Some(range) = sourcepos_to_byte_range(line_starts, body.len(), &data.sourcepos)
            else {
                return;
            };
            let prefix = continuation_prefix_from_ancestors(ancestors);
            let paragraph_max = if max_width == 0 {
                0
            } else {
                max_width.saturating_sub(prefix.len())
            };
            // The paragraph's source bytes include the `>` continuation
            // markers on continuation lines (block quotes only — list-item
            // continuation is plain whitespace that `collapse_whitespace`
            // already eats). Strip them before sembr.
            let bq_depth = block_quote_depth(ancestors);
            let cleaned = strip_block_quote_markers(&body[range.clone()], bq_depth);
            // Atomic-range protection from the inline AST. The outer AST's
            // sourcepos values are in *body* coordinates, which align with
            // `cleaned` only when no stripping happened (block-quote depth
            // zero). For nested-in-blockquote paragraphs, the cleaner
            // approach is to re-parse `cleaned` as a standalone markdown
            // fragment and read inline sourcepos from that AST — those
            // values are in cleaned coordinates by construction.
            let atomic_ranges = if bq_depth == 0 {
                collect_inline_atomic_ranges(node, range.start, line_starts, body.len())
            } else {
                collect_inline_atomic_ranges_from_text(&cleaned)
            };
            let raw = reflow_paragraph(&cleaned, &atomic_ranges, paragraph_max);
            let text = if prefix.is_empty() {
                raw
            } else {
                raw.replace('\n', &format!("\n{prefix}"))
            };
            out.push(Replacement { range, text });
            // Paragraph's children are inlines, not blocks — no further
            // recursion needed.
        }
        NodeValue::Document
        | NodeValue::BlockQuote
        | NodeValue::List(_)
        | NodeValue::Item(_)
        | NodeValue::TaskItem(_)
        | NodeValue::Alert(_)
        | NodeValue::MultilineBlockQuote(_)
        | NodeValue::FootnoteDefinition(_)
        | NodeValue::BlockDirective(_) => {
            ancestors.push(node);
            for child in node.children() {
                collect_paragraphs(child, ancestors, out, body, line_starts, max_width);
            }
            ancestors.pop();
        }
        _ => {
            // Unsupported container or non-reflowable leaf block. Preserve
            // verbatim by not descending; any nested paragraphs inside (e.g.
            // inside a FootnoteDefinition or DescriptionList) stay as-is.
        }
    }
}

/// Build the continuation-prefix string from the chain of ancestor nodes
/// surrounding a paragraph.
/// Each supported container contributes a fragment; unsupported ancestors
/// contribute nothing.
fn continuation_prefix_from_ancestors(ancestors: &[&AstNode<'_>]) -> String {
    let mut prefix = String::new();
    for (i, ancestor) in ancestors.iter().enumerate() {
        match &ancestor.data().value {
            // Alert (GFM `> [!NOTE]`) shares BlockQuote's per-line `>`
            // prefix. MultilineBlockQuote (`>>>`) has its delimiters on
            // their own lines and unprefixed content inside, so it falls
            // through to the wildcard arm and contributes nothing.
            NodeValue::BlockQuote | NodeValue::Alert(_) => prefix.push_str("> "),
            NodeValue::Item(node_list) => {
                // `padding` is the marker width including the trailing space,
                // per comrak's NodeList documentation.
                for _ in 0..node_list.padding {
                    prefix.push(' ');
                }
            }
            // Footnote definition: continuation indent is fixed at 4 spaces
            // per CommonMark's footnotes extension.
            NodeValue::FootnoteDefinition(_) => prefix.push_str("    "),
            NodeValue::TaskItem(_) => {
                // TaskItem has no padding of its own. Inherit the parent
                // List's padding (marker width) and add 4 for `[X] `.
                if i > 0
                    && let NodeValue::List(node_list) = &ancestors[i - 1].data().value
                {
                    for _ in 0..node_list.padding {
                        prefix.push(' ');
                    }
                }
                prefix.push_str("    ");
            }
            _ => {}
        }
    }
    prefix
}

/// Returns `true` if the paragraph has at least one inline [`LineBreak`] (a
/// markdown hard break) anywhere in its subtree.
///
/// Hard breaks can live nested inside inline containers (emphasis, link text,
/// etc.).
/// A direct-children check misses those: the paragraph would then reflow,
/// `walk_inline_for_atomic_ranges` would treat the outer span as atomic, and
/// `fold_line_breaks` would collapse the hard break into a space.
///
/// [`LineBreak`]: NodeValue::LineBreak
fn has_hard_line_break<'a>(paragraph: &'a AstNode<'a>) -> bool {
    paragraph
        .descendants()
        .any(|n| matches!(n.data().value, NodeValue::LineBreak))
}

/// Count ancestors that introduce a per-line `>` marker (regular block quotes
/// and GFM alerts), so we know how many layers of `>` to strip from
/// continuation lines before sembr.
fn block_quote_depth(ancestors: &[&AstNode<'_>]) -> usize {
    ancestors
        .iter()
        .filter(|a| matches!(a.data().value, NodeValue::BlockQuote | NodeValue::Alert(_)))
        .count()
}

/// Remove leading `>` block-quote markers from each line after the first, up to
/// `depth` layers per line.
/// Leaves line 0 alone (its prefix is outside the paragraph's sourcepos range
/// already).
///
/// Tolerant of both ` >  ` and bare `>` markers, and of leading whitespace
/// before each marker (CommonMark allows up to 3 spaces of indent).
fn strip_block_quote_markers(text: &str, depth: usize) -> String {
    if depth == 0 {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i == 0 {
            out.push_str(line);
            continue;
        }
        out.push('\n');
        let mut rest = line;
        for _ in 0..depth {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix("> ") {
                rest = after;
            } else if let Some(after) = rest.strip_prefix('>') {
                rest = after;
            } else {
                break;
            }
        }
        out.push_str(rest);
    }
    out
}

/// Walk a [`Paragraph`]'s inline subtree and collect byte ranges (in the
/// original body) for inline elements that must be treated as indivisible
/// during sentence segmentation.
/// The set covers all emphasis variants (`Emph`, `Strong`, `Strikethrough`),
/// inline code (`Code`), links and images, raw HTML, math spans, footnote
/// references, and wikilinks.
/// Once a node matches, recursion stops at that subtree: nested emphasis inside
/// a link is already covered by the outer link's range.
///
/// [`Paragraph`]: NodeValue::Paragraph
fn collect_inline_atomic_ranges<'a>(
    paragraph: &'a AstNode<'a>,
    paragraph_start: usize,
    line_starts: &[usize],
    body_len: usize,
) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for child in paragraph.children() {
        walk_inline_for_atomic_ranges(child, paragraph_start, line_starts, body_len, &mut out);
    }
    out
}

/// Re-parse `text` as a standalone markdown fragment and collect inline atomic
/// ranges from any paragraphs found inside.
/// Used for paragraphs nested in block quotes: the outer AST's sourcepos values
/// are in body coordinates that drifted out of alignment when
/// `strip_block_quote_markers` removed the per-line `>` prefixes, so the
/// simplest correct thing is to re-parse the stripped text and read sourcepos
/// from that fresh AST, where values are in `text` coordinates by construction.
///
/// Cost: one extra comrak parse per block-quote-nested paragraph.
/// Block quotes are rare in doc comments and markdown files alike, so this is
/// acceptable.
fn collect_inline_atomic_ranges_from_text(text: &str) -> Vec<Range<usize>> {
    let arena = Arena::new();
    let options = comrak_options();
    let root = comrak::parse_document(&arena, text, &options);
    let line_starts = line_start_offsets(text);

    let mut out = Vec::new();
    walk_paragraphs_for_atomic_ranges(root, &line_starts, text.len(), &mut out);
    out
}

/// Descend the re-parsed AST and collect inline atomic ranges from every
/// paragraph encountered.
/// Mirrors the descend list in `collect_paragraphs` so we don't miss a
/// paragraph nested in a list item or alert inside the stripped block-quote
/// content (e.g.
/// `> - foo. bar.`).
fn walk_paragraphs_for_atomic_ranges<'a>(
    node: &'a AstNode<'a>,
    line_starts: &[usize],
    text_len: usize,
    out: &mut Vec<Range<usize>>,
) {
    let data = node.data();
    if matches!(data.value, NodeValue::Paragraph) {
        for child in node.children() {
            walk_inline_for_atomic_ranges(child, 0, line_starts, text_len, out);
        }
        return;
    }
    for child in node.children() {
        walk_paragraphs_for_atomic_ranges(child, line_starts, text_len, out);
    }
}

fn walk_inline_for_atomic_ranges<'a>(
    node: &'a AstNode<'a>,
    paragraph_start: usize,
    line_starts: &[usize],
    body_len: usize,
    out: &mut Vec<Range<usize>>,
) {
    let data = node.data();
    let is_atomic = matches!(
        data.value,
        NodeValue::Emph
            | NodeValue::Strong
            | NodeValue::Strikethrough
            | NodeValue::Code(_)
            | NodeValue::Link(_)
            | NodeValue::Image(_)
            | NodeValue::HtmlInline(_)
            | NodeValue::Math(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::WikiLink(_)
    );

    if is_atomic {
        if let Some(range) = sourcepos_to_byte_range(line_starts, body_len, &data.sourcepos)
            && let Some(rel_start) = range.start.checked_sub(paragraph_start)
            && let Some(rel_end) = range.end.checked_sub(paragraph_start)
        {
            out.push(rel_start..rel_end);
        }
        // Outer span covers any nested inlines; no further recursion.
        return;
    }

    for child in node.children() {
        walk_inline_for_atomic_ranges(child, paragraph_start, line_starts, body_len, out);
    }
}

/// Reflow a single paragraph of prose: semantic line breaks (one sentence per
/// line) plus an optional `max_width` safety net.
///
/// `max_width == 0` disables width wrapping.
/// With width wrapping on, individual tokens that exceed the width are kept
/// intact rather than split mid-token (URLs, paths, identifiers stay whole).
#[must_use]
pub fn reflow_paragraph(
    paragraph: &str,
    atomic_ranges: &[Range<usize>],
    max_width: usize,
) -> String {
    let sentences = split_sentences(paragraph, atomic_ranges);
    if sentences.is_empty() {
        return String::new();
    }

    if max_width == 0 {
        return sentences.join("\n");
    }

    let wrap_options = textwrap::Options::new(max_width)
        .break_words(false)
        .word_splitter(WordSplitter::NoHyphenation);

    sentences
        .iter()
        .map(|s| textwrap::fill(s, &wrap_options))
        .collect::<Vec<_>>()
        .join("\n")
}

struct Replacement {
    range: Range<usize>,
    text: String,
}

/// Convert a comrak [`Sourcepos`] (1-based line, 1-based byte column,
/// end-inclusive) into a half-open byte range.
///
/// Returns `None` if the coordinates fall outside `body_len` — a defensive
/// guard against any sourcepos drift we haven't observed but shouldn't rely on
/// the absence of.
///
/// [`Sourcepos`]: comrak::nodes::Sourcepos
fn sourcepos_to_byte_range(
    line_starts: &[usize],
    body_len: usize,
    sp: &comrak::nodes::Sourcepos,
) -> Option<Range<usize>> {
    let start_line = sp.start.line.checked_sub(1)?;
    let end_line = sp.end.line.checked_sub(1)?;
    let start_line_offset = *line_starts.get(start_line)?;
    let end_line_offset = *line_starts.get(end_line)?;

    let start = start_line_offset.checked_add(sp.start.column.saturating_sub(1))?;
    let end = end_line_offset.checked_add(sp.end.column)?;

    if start > end || end > body_len {
        return None;
    }
    Some(start..end)
}

/// Byte offset of the first character of each line, with an implicit
/// `line_starts[0] == 0`.
fn line_start_offsets(s: &str) -> Vec<usize> {
    let mut offsets = vec![0_usize];
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            offsets.push(i + 1);
        }
    }
    offsets
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
