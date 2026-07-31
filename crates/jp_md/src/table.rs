//! ANSI-aware table formatting for terminal output.
//!
//! This module renders comrak `Table` AST nodes as aligned, padded tables with
//! proper column alignment markers.
//! It handles ANSI escape sequences in cell content correctly by computing
//! visual width (ignoring invisible escape bytes) for padding calculations.
//!
//! Column widths are fitted to the width available on the output line: a table
//! that fits keeps its natural widths, and one that does not has its widest
//! columns narrowed until the whole table fits.
//! Cell content that exceeds its fitted column width is word-wrapped across
//! multiple visual lines, preserving ANSI formatting state across line breaks.
//! A line that continues the row above opens with [`CONTINUATION_EDGE`] rather
//! than `|`, so a wrapped row reads as one row instead of several.
//!
//! # Usage
//!
//! Called from the terminal renderer when it encounters a `Table` node.
//! The renderer passes the table node and the number of columns available, and
//! receives a fully formatted string that it writes directly to output.

use std::{cmp::min, fmt::Write as _};

use comrak::nodes::{NodeValue, TableAlignment};

use crate::{
    ansi::{self, AnsiState, RESET, Segment},
    render::{RenderOptions, TerminalFormatter},
};

/// Type alias for comrak AST node references.
type Node<'a> = &'a comrak::nodes::AstNode<'a>;

/// Minimum visual width for a table column.
///
/// Keeps the separator row (`---`) readable when the available width forces the
/// narrowest possible layout.
const MIN_COLUMN_WIDTH: usize = 3;

/// Visual width each column adds on top of its content: the leading `|` and the
/// space on either side of the cell.
const COLUMN_CHROME: usize = 3;

/// Opens a line that continues the row above rather than starting a new one.
///
/// Only the row's opening delimiter takes this glyph; the inner and trailing
/// `|` stay put.
/// GFM treats a row's leading `|` as optional, so a continuation line pasted
/// into a markdown document still splits into the right columns on the pipes
/// that remain.
const CONTINUATION_EDGE: char = '┆';

/// Marks a header cell cut short because it did not fit its column.
const TRUNCATION_MARKER: char = '…';

/// Options for table formatting.
pub struct TableOptions {
    /// Upper bound on the visual width of any single column.
    ///
    /// Cells exceeding this width are word-wrapped across multiple rows.
    /// `0` means unbounded.
    /// A column can still end up narrower than this, when the table would
    /// otherwise not fit the available width.
    pub max_column_width: usize,

    /// Whether a line continuing the row above opens with [`CONTINUATION_EDGE`]
    /// instead of `|`.
    pub continuation_edge: bool,
}

impl TableOptions {
    /// Create a new `TableOptions` with the given column width, marking the
    /// continuation lines of wrapped rows.
    pub const fn new(max_column_width: usize) -> Self {
        Self {
            max_column_width,
            continuation_edge: true,
        }
    }

    /// Set whether continuation lines are marked.
    #[must_use]
    pub const fn continuation_edge(mut self, enabled: bool) -> Self {
        self.continuation_edge = enabled;
        self
    }
}

/// Format a comrak `Table` node into an aligned, ANSI-styled string.
///
/// `budget` is the number of visual columns available for the rendered table,
/// borders included; `None` means unbounded.
/// No rendered line exceeds it unless the table has too many columns to fit
/// even at [`MIN_COLUMN_WIDTH`].
///
/// Returns `None` if the node isn't a valid table structure.
///
/// The function:
///
/// 1. Walks the table's children to extract rows and cells.
/// 2. Renders each cell's inline content using the terminal renderer (with
///    `width: 0` to disable wrapping inside cells).
/// 3. Computes natural visual column widths (ignoring ANSI bytes) and fits them
///    to `budget`.
/// 4. Word-wraps cells that exceed their fitted column width, opening each
///    continuation line with [`CONTINUATION_EDGE`].
/// 5. Pads and aligns cells according to the table's alignment markers.
pub fn format_table(
    node: Node<'_>,
    options: RenderOptions<'_>,
    budget: Option<usize>,
) -> Option<String> {
    let (alignments, rows) = extract_table(node, options)?;

    // Compute the width each column would take if nothing constrained it.
    let num_cols = alignments.len();
    let mut natural = vec![0_usize; num_cols];

    for row in &rows {
        for (col, cell) in row.iter().enumerate() {
            if col < num_cols {
                let vw = ansi::visual_width(&cell.rendered);
                natural[col] = natural[col].max(vw);
            }
        }
    }

    let col_widths = fit_columns(&natural, options.table_options.max_column_width, budget);

    // Render the table.
    let mut out = String::new();

    for (row_idx, row) in rows.iter().enumerate() {
        let is_header = row_idx == 0;

        // Wrap each cell's content into lines that fit the column width. The
        // header is truncated to a single line instead: wrapping it would push
        // the separator off the second line, and a markdown parser reading this
        // output then promotes the header's own tail to the header row.
        let wrapped: Vec<Vec<String>> = (0..num_cols)
            .map(|col| {
                let content = row.get(col).map_or("", |c| c.rendered.as_str());
                if is_header {
                    vec![truncate_to_visual_width(content, col_widths[col])]
                } else {
                    wrap_to_visual_width(content, col_widths[col])
                }
            })
            .collect();

        let max_lines = wrapped.iter().map(Vec::len).max().unwrap_or(1);

        for line_idx in 0..max_lines {
            if line_idx == 0 || !options.table_options.continuation_edge {
                out.push('|');
            } else {
                out.push(CONTINUATION_EDGE);
            }
            for (col, col_lines) in wrapped.iter().enumerate() {
                if col >= num_cols {
                    break;
                }
                let align = alignments.get(col).copied().unwrap_or(TableAlignment::None);
                let content = col_lines.get(line_idx).map_or("", String::as_str);
                let padded = pad_cell(content, col_widths[col], align);
                let _ = write!(out, " {padded} |");
            }
            out.push('\n');
        }

        // Separator line after header row.
        if is_header {
            out.push('|');
            for (col, align) in alignments.iter().enumerate() {
                let w = col_widths[col];
                let sep = match align {
                    TableAlignment::Left => format!(":{}|", "-".repeat(w + 1)),
                    TableAlignment::Right => format!("{}:|", "-".repeat(w + 1)),
                    TableAlignment::Center => format!(":{}:|", "-".repeat(w)),
                    TableAlignment::None => format!("{}|", "-".repeat(w + 2)),
                };
                let _ = write!(out, "{sep}");
            }
            out.push('\n');
        }
    }

    Some(out)
}

/// Fit natural column widths into `budget` visual columns.
///
/// Each width is first clamped to `max_column_width` (`0` = unbounded) and
/// raised to [`MIN_COLUMN_WIDTH`]. If the result fits `budget` (`None` =
/// unbounded), it is returned as-is.
///
/// Otherwise the budget is distributed max-min fair: every column that asks for
/// no more than an equal share keeps its full width and donates the remainder
/// to the columns that want more, repeatedly, until only columns wider than the
/// share are left.
/// So a table of four short columns and one prose column spends the surplus on
/// the prose column instead of narrowing all five to the same width.
///
/// A table with more columns than `budget` can hold at [`MIN_COLUMN_WIDTH`] is
/// laid out at that minimum and overflows: no distribution can save it, and a
/// one-character column is less useful than an overflowing table.
/// `Some(0)` — a known budget with nothing left over, such as a table nested
/// so deeply that its prefix consumes the terminal — is that same minimum
/// layout, not an unbounded one.
fn fit_columns(natural: &[usize], max_column_width: usize, budget: Option<usize>) -> Vec<usize> {
    let count = natural.len();
    let mut widths: Vec<usize> = natural
        .iter()
        .map(|&w| {
            let capped = if max_column_width > 0 {
                min(w, max_column_width)
            } else {
                w
            };
            capped.max(MIN_COLUMN_WIDTH)
        })
        .collect();

    let Some(budget) = budget else {
        return widths;
    };

    // The trailing `|` closes the last column; every column adds its own chrome.
    let chrome = count * COLUMN_CHROME + 1;
    let content_budget = budget.saturating_sub(chrome);
    if widths.iter().sum::<usize>() <= content_budget {
        return widths;
    }

    let mut flexible = vec![true; count];
    let mut remaining_budget = content_budget;
    let mut remaining_cols = count;

    // Settle the columns that fit within an equal share, freeing their surplus
    // for the rest. Each pass raises the share, so it terminates once no column
    // settles.
    while remaining_cols > 0 {
        let share = remaining_budget / remaining_cols;
        let mut settled = false;
        for col in 0..count {
            if !flexible[col] || widths[col] > share {
                continue;
            }
            flexible[col] = false;
            remaining_budget -= widths[col];
            remaining_cols -= 1;
            settled = true;
        }
        if !settled {
            break;
        }
    }

    if let Some(share) = remaining_budget.checked_div(remaining_cols) {
        // Hand the division remainder to the leftmost flexible columns, so the
        // layout is deterministic and spends the whole budget.
        let mut extra = remaining_budget % remaining_cols;
        for col in 0..count {
            if !flexible[col] {
                continue;
            }
            let bonus = usize::from(extra > 0);
            extra -= bonus;
            widths[col] = (share + bonus).max(MIN_COLUMN_WIDTH);
        }
    }

    widths
}

/// A rendered table cell.
struct RenderedCell {
    /// The cell content with ANSI escapes included.
    rendered: String,
}

/// Extract the table structure from a `Table` AST node.
///
/// Returns the alignment list and a 2D vector of rendered cells.
fn extract_table(
    node: Node<'_>,
    options: RenderOptions<'_>,
) -> Option<(Vec<TableAlignment>, Vec<Vec<RenderedCell>>)> {
    let alignments = match node.data().value {
        NodeValue::Table(ref nt) => nt.alignments.clone(),
        _ => return None,
    };

    let mut rows: Vec<Vec<RenderedCell>> = Vec::new();

    for row_node in node.children() {
        if !matches!(row_node.data().value, NodeValue::TableRow(..)) {
            continue;
        }

        let mut cells = Vec::new();
        for cell_node in row_node.children() {
            if !matches!(cell_node.data().value, NodeValue::TableCell) {
                continue;
            }

            let rendered = render_cell_content(cell_node, options);
            cells.push(RenderedCell { rendered });
        }
        rows.push(cells);
    }

    Some((alignments, rows))
}

/// Render the inline content of a table cell using the terminal formatter.
///
/// Uses `width: 0` to disable line wrapping inside cells.
fn render_cell_content(cell_node: Node<'_>, options: RenderOptions<'_>) -> String {
    let mut buf = String::new();
    {
        // Use TerminalFormatter to render the cell's children.
        //
        // Wrapping and indent are handled at the cell level, so the nested
        // renderer runs with both disabled; everything else (theme,
        // backgrounds, HR style) is inherited.
        //
        // Note: `TerminalFormatter` emits a default background escape if one is
        // set.
        let cell_options = RenderOptions {
            width: 0,
            indent: 0,
            ..options
        };
        let mut formatter = TerminalFormatter::new(cell_node, cell_options, &mut buf);

        // format() visits the node and its children.
        //
        // `NodeValue::TableCell` is handled by the default case in format_node,
        // which visits children.
        let _ = formatter.format(cell_node);
    }

    if buf.ends_with('\n') {
        buf.pop();
    }

    buf
}

/// Pad a cell's rendered content to the target width with the given alignment.
fn pad_cell(content: &str, target_width: usize, alignment: TableAlignment) -> String {
    let vw = ansi::visual_width(content);
    let pad = target_width.saturating_sub(vw);

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

/// Truncate a string (possibly containing ANSI escapes) to a maximum visual
/// width, marking the cut with [`TRUNCATION_MARKER`].
///
/// Content that already fits, and any content at all when `max_width` is `0`,
/// is returned unchanged.
/// The marker takes the last column, so `max_width` still bounds the result.
/// ANSI state left open at the cut is closed, so styling does not leak into the
/// rest of the line.
fn truncate_to_visual_width(content: &str, max_width: usize) -> String {
    if max_width == 0 || ansi::visual_width(content) <= max_width {
        return content.to_string();
    }

    let keep = max_width - 1;
    let mut out = String::new();
    let mut state = AnsiState::default();

    'segments: for segment in ansi::segments(content) {
        let text = match segment {
            Segment::Escape(escape) => {
                state.update(escape);
                out.push_str(escape);
                continue;
            }
            Segment::Text(text) => text,
        };

        for c in text.chars() {
            out.push(c);
            if ansi::visual_width(&out) > keep {
                out.pop();
                break 'segments;
            }
        }
    }

    // An escape never ends in a space, so this only trims visible padding.
    while out.ends_with(' ') {
        out.pop();
    }

    out.push(TRUNCATION_MARKER);
    if state.is_active() {
        out.push_str(RESET);
    }

    out
}

/// Word-wrap a string (possibly containing ANSI escapes) to a maximum visual
/// width.
///
/// Returns a `Vec` of lines, each fitting within `max_width` visible
/// characters.
/// Words are split at space boundaries; a single word longer than `max_width`
/// is hard-broken at the character level.
///
/// ANSI escape state is properly closed at each line break and re-opened on the
/// continuation line.
fn wrap_to_visual_width(content: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || ansi::visual_width(content) <= max_width {
        return vec![content.to_string()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    // State committed to `current` — updated only when a word is flushed.
    let mut state = AnsiState::default();

    // Accumulate the current word (visible chars + interspersed ANSI).
    let mut word = String::new();

    for segment in ansi::segments(content) {
        let text = match segment {
            // Append the escape to the word buffer (not yet committed).
            Segment::Escape(escape) => {
                word.push_str(escape);
                continue;
            }
            Segment::Text(text) => text,
        };

        for c in text.chars() {
            if c != ' ' {
                word.push(c);
                continue;
            }

            // Flush the pending word.
            flush_word(&mut lines, &mut current, &mut state, &word, max_width);
            word.clear();

            // Add space separator if room remains on the line.
            let vw = ansi::visual_width(&current);
            if vw > 0 && vw < max_width {
                current.push(' ');
            } else if vw >= max_width {
                // Line is full — break before the space.
                finalize_line(&mut lines, &mut current, &state);
                current = state.restore_sequence();
            }
        }
    }

    // Flush any remaining word.
    if !word.is_empty() {
        flush_word(&mut lines, &mut current, &mut state, &word, max_width);
    }

    if !current.is_empty() {
        lines.push(current);
    }

    // Ensure we return at least one (possibly empty) line.
    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Flush a completed word onto the current line, breaking if needed.
///
/// Uses `visual_width` on accumulated strings rather than per-character
/// tracking, so multi-codepoint sequences (VS16 emoji, ZWJ) are measured
/// correctly.
fn flush_word(
    lines: &mut Vec<String>,
    current: &mut String,
    state: &mut AnsiState,
    word: &str,
    max_width: usize,
) {
    let word_vw = ansi::visual_width(word);

    if word_vw == 0 {
        // Word contains only ANSI escapes — append without consuming width.
        current.push_str(word);
        state.update_from_str(word);
        return;
    }

    let current_vw = ansi::visual_width(current);

    // Case 1: word fits on the current line.
    if current_vw + word_vw <= max_width {
        current.push_str(word);
        state.update_from_str(word);
        return;
    }

    // Case 2: doesn't fit, but the word fits on a fresh line.
    if word_vw <= max_width {
        // Trim trailing space from the current line.
        if current.ends_with(' ') {
            current.pop();
        }
        if current_vw > 0 {
            finalize_line(lines, current, state);
            *current = state.restore_sequence();
        }
        current.push_str(word);
        state.update_from_str(word);
        return;
    }

    // Case 3: single word exceeds max_width — hard-break it.
    if current_vw > 0 {
        if current.ends_with(' ') {
            current.pop();
        }
        finalize_line(lines, current, state);
        *current = state.restore_sequence();
    }
    hard_break_into(lines, current, state, word, max_width);
}

/// Close the current line: emit a reset if ANSI state is active, then push the
/// line and prepare `current` for the next line.
fn finalize_line(lines: &mut Vec<String>, current: &mut String, state: &AnsiState) {
    if state.is_active() {
        current.push_str(RESET);
    }
    lines.push(std::mem::take(current));
}

/// Hard-break a word that exceeds `max_width` across multiple lines, preserving
/// ANSI escape state.
///
/// Uses `visual_width` on the accumulated line to decide break points, so
/// multi-codepoint emoji sequences are measured correctly.
fn hard_break_into(
    lines: &mut Vec<String>,
    current: &mut String,
    state: &mut AnsiState,
    word: &str,
    max_width: usize,
) {
    for segment in ansi::segments(word) {
        let text = match segment {
            Segment::Escape(escape) => {
                state.update(escape);
                current.push_str(escape);
                continue;
            }
            Segment::Text(text) => text,
        };

        for c in text.chars() {
            current.push(c);
            if ansi::visual_width(current) > max_width {
                current.pop();
                finalize_line(lines, current, state);
                *current = state.restore_sequence();
                current.push(c);
            }
        }
    }
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
