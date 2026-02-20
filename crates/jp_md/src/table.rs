//! ANSI-aware table formatting for terminal output.
//!
//! This module renders comrak `Table` AST nodes as aligned, padded tables with
//! proper column alignment markers. It handles ANSI escape sequences in cell
//! content correctly by computing visual width (ignoring invisible escape
//! bytes) for padding calculations.
//!
//! Cell content that exceeds the configured maximum column width is
//! word-wrapped across multiple visual rows, preserving ANSI formatting state
//! across line breaks.
//!
//! # Usage
//!
//! Called from the terminal renderer when it encounters a `Table` node. The
//! renderer passes the table node and receives a fully formatted string that it
//! writes directly to output.

use std::{cmp::min, fmt::Write as _};

use comrak::nodes::{NodeValue, TableAlignment};
use syntect::highlighting::Theme;

use crate::{
    ansi::{self, AnsiState, RESET},
    format::DefaultBackground,
    render::{HrOptions, TerminalFormatter},
};

/// Type alias for comrak AST node references.
type Node<'a> = &'a comrak::nodes::AstNode<'a>;

/// Options for table formatting.
pub struct TableOptions {
    /// Maximum visual width for any single column.
    ///
    /// Cells exceeding this width are word-wrapped across multiple rows. `0`
    /// means unlimited.
    pub max_column_width: usize,
}

impl TableOptions {
    /// Create a new `TableOptions` with the given column width.
    pub const fn new(max_column_width: usize) -> Self {
        Self { max_column_width }
    }
}

/// Format a comrak `Table` node into an aligned, ANSI-styled string.
///
/// Returns `None` if the node isn't a valid table structure.
///
/// The function:
/// 1. Walks the table's children to extract rows and cells.
/// 2. Renders each cell's inline content using the terminal renderer (with
///    `width: 0` to disable wrapping inside cells).
/// 3. Computes visual column widths (ignoring ANSI bytes).
/// 4. Word-wraps cells that exceed the maximum column width.
/// 5. Pads and aligns cells according to the table's alignment markers.
pub fn format_table(
    node: Node<'_>,
    options: &TableOptions,
    hr_options: &HrOptions,
    theme: &Theme,
    default_background: Option<&DefaultBackground>,
) -> Option<String> {
    let (alignments, rows) = extract_table(node, options, hr_options, theme, default_background)?;

    // Compute visual widths for each column.
    let num_cols = alignments.len();

    // minimum 3 for separator "---"
    let mut col_widths = vec![3_usize; num_cols];

    for row in &rows {
        for (col, cell) in row.iter().enumerate() {
            if col < num_cols {
                let vw = ansi::visual_width(&cell.rendered);
                col_widths[col] = col_widths[col].max(vw);
            }
        }
    }

    // Apply max column width cap.
    if options.max_column_width > 0 {
        for w in &mut col_widths {
            *w = min(*w, options.max_column_width);
        }
    }

    // Render the table.
    let mut out = String::new();

    for (row_idx, row) in rows.iter().enumerate() {
        // Wrap each cell's content into lines that fit the column width.
        let wrapped: Vec<Vec<String>> = (0..num_cols)
            .map(|col| {
                let content = row.get(col).map_or("", |c| c.rendered.as_str());
                if options.max_column_width > 0 {
                    wrap_to_visual_width(content, col_widths[col])
                } else {
                    vec![content.to_string()]
                }
            })
            .collect();

        let max_lines = wrapped.iter().map(Vec::len).max().unwrap_or(1);

        for line_idx in 0..max_lines {
            out.push('|');
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
        if row_idx == 0 {
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
    options: &TableOptions,
    hr_options: &HrOptions,
    theme: &Theme,
    default_background: Option<&DefaultBackground>,
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

            let rendered =
                render_cell_content(cell_node, options, hr_options, theme, default_background);
            cells.push(RenderedCell { rendered });
        }
        rows.push(cells);
    }

    Some((alignments, rows))
}

/// Render the inline content of a table cell using the terminal formatter.
///
/// Uses `width: 0` to disable line wrapping inside cells.
fn render_cell_content(
    cell_node: Node<'_>,
    options: &TableOptions,
    hr_options: &HrOptions,
    theme: &Theme,
    default_background: Option<&DefaultBackground>,
) -> String {
    let mut buf = String::new();
    {
        // Use TerminalFormatter to render the cell's children.
        //
        // We use width=0 to disable wrapping (we handle it at the cell level).
        // We pass the cell node itself — `TerminalFormatter` will visit its
        // children.
        //
        // Note: `TerminalFormatter` emits a default background escape if one is
        // set.
        let mut formatter = TerminalFormatter::new(
            cell_node,
            0,
            options,
            hr_options,
            theme,
            default_background,
            &mut buf,
        );

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

/// Word-wrap a string (possibly containing ANSI escapes) to a maximum visual
/// width.
///
/// Returns a `Vec` of lines, each fitting within `max_width` visible
/// characters. Words are split at space boundaries; a single word longer than
/// `max_width` is hard-broken at the character level.
///
/// ANSI escape state is properly closed at each line break and re-opened on the
/// continuation line.
fn wrap_to_visual_width(content: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || ansi::visual_width(content) <= max_width {
        return vec![content.to_string()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_vw: usize = 0;

    // State committed to `current` — updated only when a word is flushed.
    let mut state = AnsiState::default();

    // Accumulate the current word (visible chars + interspersed ANSI).
    let mut word = String::new();
    let mut word_vw: usize = 0;

    let mut in_escape = false;
    let mut escape_buf = String::new();

    for c in content.chars() {
        if in_escape {
            escape_buf.push(c);
            if c.is_ascii_alphabetic() || c == '~' {
                in_escape = false;
                // Append the escape to the word buffer (not yet committed).
                word.push_str(&escape_buf);
                escape_buf.clear();
            }
            continue;
        }

        if c == '\x1b' {
            in_escape = true;
            escape_buf.clear();
            escape_buf.push(c);
            continue;
        }

        if c == ' ' {
            // Flush the pending word.
            flush_word(
                &mut lines,
                &mut current,
                &mut current_vw,
                &mut state,
                &word,
                word_vw,
                max_width,
            );
            word.clear();
            word_vw = 0;

            // Add space separator if room remains on the line.
            if current_vw > 0 && current_vw < max_width {
                current.push(' ');
                current_vw += 1;
            } else if current_vw >= max_width {
                // Line is full — break before the space.
                finalize_line(&mut lines, &mut current, &state);
                current = state.restore_sequence();
                current_vw = 0;
            }
            continue;
        }

        // Visible character.
        word.push(c);
        word_vw += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    }

    // Flush any remaining word.
    if !word.is_empty() || word_vw > 0 {
        flush_word(
            &mut lines,
            &mut current,
            &mut current_vw,
            &mut state,
            &word,
            word_vw,
            max_width,
        );
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
fn flush_word(
    lines: &mut Vec<String>,
    current: &mut String,
    current_vw: &mut usize,
    state: &mut AnsiState,
    word: &str,
    word_vw: usize,
    max_width: usize,
) {
    if word_vw == 0 {
        // Word contains only ANSI escapes — append without consuming width.
        current.push_str(word);
        state.update_from_str(word);
        return;
    }

    // Case 1: word fits on the current line.
    if *current_vw + word_vw <= max_width {
        current.push_str(word);
        *current_vw += word_vw;
        state.update_from_str(word);
        return;
    }

    // Case 2: doesn't fit, but the word fits on a fresh line.
    if word_vw <= max_width {
        // Trim trailing space from the current line.
        if current.ends_with(' ') {
            current.pop();
        }
        if *current_vw > 0 {
            finalize_line(lines, current, state);
            *current = state.restore_sequence();
            *current_vw = 0;
        }
        current.push_str(word);
        *current_vw = word_vw;
        state.update_from_str(word);
        return;
    }

    // Case 3: single word exceeds max_width — hard-break it.
    if *current_vw > 0 {
        if current.ends_with(' ') {
            current.pop();
        }
        finalize_line(lines, current, state);
        *current = state.restore_sequence();
        *current_vw = 0;
    }
    hard_break_into(lines, current, current_vw, state, word, max_width);
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
fn hard_break_into(
    lines: &mut Vec<String>,
    current: &mut String,
    current_vw: &mut usize,
    state: &mut AnsiState,
    word: &str,
    max_width: usize,
) {
    let mut in_escape = false;
    let mut escape_buf = String::new();

    for c in word.chars() {
        if in_escape {
            escape_buf.push(c);
            if c.is_ascii_alphabetic() || c == '~' {
                in_escape = false;
                state.update(&escape_buf);
                current.push_str(&escape_buf);
                escape_buf.clear();
            }
            continue;
        }

        if c == '\x1b' {
            in_escape = true;
            escape_buf.clear();
            escape_buf.push(c);
            continue;
        }

        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if cw > 0 && *current_vw + cw > max_width {
            finalize_line(lines, current, state);
            *current = state.restore_sequence();
            *current_vw = 0;
        }

        current.push(c);
        *current_vw += cw;
    }
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
