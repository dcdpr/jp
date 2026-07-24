use comfy_table::{Cell, CellAlignment, Row, Table};

pub const EMPTY: &str = "                   ";
pub const UTF8_FULL: &str = "││──├──┤     ──╭╮╰╯";

/// A value rendered in a key-value details view.
#[derive(Debug, Clone)]
pub enum DetailValue {
    /// A single value.
    Scalar(String),

    /// A list of items: a bulleted multi-line cell in the pretty view, one row
    /// per item in markdown, and a JSON array in the JSON views.
    List(Vec<DetailItem>),
}

/// An item in a [`DetailValue::List`].
///
/// Carries a human-facing `text` form (pretty + markdown) and a structured
/// `json` form (JSON views) so the two can differ: a list can read as `cmd
/// (Current Date): cmd://...` in the terminal while serializing as an object in
/// JSON.
#[derive(Debug, Clone)]
pub struct DetailItem {
    pub text: String,
    pub json: serde_json::Value,
}

impl DetailItem {
    /// An item with distinct text and JSON forms.
    #[must_use]
    pub fn new(text: impl Into<String>, json: serde_json::Value) -> Self {
        Self {
            text: text.into(),
            json,
        }
    }

    /// An item whose text and JSON forms are the same plain string.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            json: serde_json::Value::String(text.clone()),
            text,
        }
    }
}

/// A labeled row in a key-value details view.
///
/// A `None` label produces a label-less row: a single value column in the
/// pretty and markdown views.
/// Listing commands use this to render a titled column of values without keys.
#[derive(Debug, Clone)]
pub struct DetailRow {
    pub label: Option<String>,
    pub value: DetailValue,
}

impl DetailRow {
    /// A labeled single-value row.
    #[must_use]
    pub fn scalar(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
            value: DetailValue::Scalar(value.into()),
        }
    }

    /// A labeled multi-value row.
    #[must_use]
    pub fn list(label: impl Into<String>, values: Vec<DetailItem>) -> Self {
        Self {
            label: Some(label.into()),
            value: DetailValue::List(values),
        }
    }

    /// A label-less single-value row.
    #[must_use]
    pub fn bare(value: impl Into<String>) -> Self {
        Self {
            label: None,
            value: DetailValue::Scalar(value.into()),
        }
    }
}

/// Render a list table with unicode box-drawing characters.
///
/// When `footer` is true, the header row is repeated at the bottom of the table
/// so it stays visible when the top has scrolled off screen.
#[must_use]
pub fn list(header: Row, rows: Vec<Row>, footer: bool) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(header);
    table.add_rows(rows);

    let rendered = table.trim_fmt();

    if !footer {
        return rendered;
    }

    // Splice a copy of the header row before the bottom border.
    // Rendered structure:
    //   [0] top border       ╭──╮
    //   [1] header content   │..│
    //   [2] separator        ├──┤
    //   [3..n-1] data rows   │..│
    //   [n] bottom border    ╰──╯
    let lines: Vec<&str> = rendered.lines().collect();
    if lines.len() < 6 {
        return rendered;
    }

    let header_content = lines[1];
    let separator = lines[2];

    let mut out =
        String::with_capacity(rendered.len() + separator.len() + header_content.len() + 2);
    for line in &lines[..lines.len() - 1] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(separator);
    out.push('\n');
    out.push_str(header_content);
    out.push('\n');

    if let Some(last) = lines.last() {
        out.push_str(last);
    }

    out
}

/// Render a list table as a pipe-delimited markdown table.
///
/// Produces output like:
///
/// ```text
/// | ID   | #  | Activity |
/// |------|---:|----------|
/// | abc  |  3 | 2m ago   |
/// ```
#[must_use]
#[expect(clippy::needless_pass_by_value)]
pub fn list_markdown(header: Row, rows: Vec<Row>) -> String {
    let all_rows: Vec<&Row> = std::iter::once(&header).chain(rows.iter()).collect();
    let col_count = max_columns(&all_rows);
    let widths = column_widths(&all_rows, col_count);

    let mut out = String::new();

    // Header row
    push_md_row(&mut out, &header, &widths, col_count);

    // Separator row
    out.push('|');
    for w in &widths {
        out.push_str(&format!(" {} |", "-".repeat(*w)));
    }
    out.push('\n');

    // Data rows
    for row in &rows {
        push_md_row(&mut out, row, &widths, col_count);
    }

    out
}

/// Render a key-value details table with no borders.
#[must_use]
pub fn details(title: Option<&str>, rows: Vec<DetailRow>) -> String {
    let mut buf = String::new();

    if let Some(title) = title {
        buf.push_str(title);
        if !rows.is_empty() {
            buf.push_str("\n\n");
        }
    }

    let mut table = Table::new();
    table.load_preset(EMPTY);
    for row in rows {
        table.add_row(detail_pretty_row(row));
    }
    buf.push_str(&table.trim_fmt());

    buf
}

/// Build a pretty (borderless table) row from a detail row.
///
/// A list value renders with the label on its own line and the items bulleted
/// beneath it (the leading newline pushes the items below the label, indented
/// into the value column).
fn detail_pretty_row(row: DetailRow) -> Row {
    let value = match row.value {
        DetailValue::Scalar(s) => s,
        DetailValue::List(items) => {
            let bullets = items
                .into_iter()
                .map(|item| format!("- {}", item.text))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n{bullets}")
        }
    };

    let mut r = Row::new();
    if let Some(label) = row.label {
        r.add_cell(Cell::new(label).set_alignment(CellAlignment::Right));
    }
    r.add_cell(Cell::new(value).set_alignment(CellAlignment::Left));
    r
}

/// Render a key-value details table as a pipe-delimited markdown table.
#[must_use]
pub fn details_markdown(title: Option<&str>, rows: Vec<DetailRow>) -> String {
    let mut buf = String::new();

    if let Some(title) = title {
        buf.push_str(title);
        if !rows.is_empty() {
            buf.push('\n');
        }
    }

    if rows.is_empty() {
        return buf;
    }

    let md_rows = detail_markdown_rows(rows);
    let row_refs: Vec<&Row> = md_rows.iter().collect();
    let col_count = max_columns(&row_refs);
    let widths = column_widths(&row_refs, col_count);

    for row in &md_rows {
        push_md_row(&mut buf, row, &widths, col_count);
    }

    buf
}

/// Flatten detail rows into pipe-table rows.
///
/// A list value expands to one row per item: the label sits on the first item's
/// row and continuation rows carry a blank label cell so the table stays
/// aligned.
fn detail_markdown_rows(rows: Vec<DetailRow>) -> Vec<Row> {
    let mut out = Vec::new();
    for row in rows {
        match row.value {
            DetailValue::Scalar(s) => out.push(md_row(row.label.as_deref(), &s)),
            DetailValue::List(items) => {
                for (idx, item) in items.iter().enumerate() {
                    let label = match (row.label.as_deref(), idx) {
                        (Some(label), 0) => Some(label),
                        (Some(_), _) => Some(""),
                        (None, _) => None,
                    };
                    out.push(md_row(label, &item.text));
                }
            }
        }
    }
    out
}

fn md_row(label: Option<&str>, value: &str) -> Row {
    let mut r = Row::new();
    if let Some(label) = label {
        r.add_cell(Cell::new(label));
    }
    r.add_cell(Cell::new(value));
    r
}

/// Find the maximum column count across all rows.
fn max_columns(rows: &[&Row]) -> usize {
    rows.iter()
        .map(|r| r.cell_iter().count())
        .max()
        .unwrap_or(0)
}

/// Compute the visual width needed for each column.
fn column_widths(rows: &[&Row], col_count: usize) -> Vec<usize> {
    let mut widths = vec![0_usize; col_count];
    for row in rows {
        for (idx, cell) in row.cell_iter().enumerate() {
            if idx < col_count {
                let content = strip_ansi_escapes::strip_str(cell.content());
                widths[idx] = widths[idx].max(content.len());
            }
        }
    }
    // Minimum width of 1 so separators look reasonable.
    for w in &mut widths {
        *w = (*w).max(1);
    }
    widths
}

/// Write a single pipe-delimited row.
fn push_md_row(out: &mut String, row: &Row, widths: &[usize], col_count: usize) {
    out.push('|');
    for idx in 0..col_count {
        let content = row
            .cell_iter()
            .nth(idx)
            .map(|c| strip_ansi_escapes::strip_str(c.content()))
            .unwrap_or_default();

        let w = widths.get(idx).copied().unwrap_or(1);
        out.push_str(&format!(" {content:<w$} |"));
    }
    out.push('\n');
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
