use comfy_table::{Cell, CellAlignment, Row, Table};

pub const EMPTY: &str = "                   ";
pub const UTF8_FULL: &str = "││──├──┤     ──╭╮╰╯";

/// A value rendered in a key-value details view.
#[derive(Debug, Clone)]
pub enum DetailValue {
    /// A single value.
    Scalar(String),

    /// A list of items: a numbered multi-line cell in the pretty view, one row
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
#[derive(Debug, Clone)]
pub struct DetailRow {
    pub label: String,
    pub value: DetailValue,
}

impl DetailRow {
    /// A labeled single-value row.
    #[must_use]
    pub fn scalar(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: DetailValue::Scalar(value.into()),
        }
    }

    /// A labeled multi-value row.
    #[must_use]
    pub fn list(label: impl Into<String>, values: Vec<DetailItem>) -> Self {
        Self {
            label: label.into(),
            value: DetailValue::List(values),
        }
    }
}

/// The body of a details view: either a record or a listing.
///
/// The variant fixes the JSON shape, so a command's output shape is a property
/// of the command rather than of the data a given invocation happens to hold.
/// A [`Fields`] view is always a JSON object and an [`Items`] view is always a
/// JSON array, including when either is empty — a consumer never needs a
/// fallback for a shape that changed under it.
///
/// The two are separate variants rather than one row type with an optional
/// label so that a record cannot acquire an unlabeled row, or a listing a
/// labeled one, which is what would make the shape data-dependent.
///
/// [`Fields`]: Details::Fields
/// [`Items`]: Details::Items
#[derive(Debug, Clone)]
pub enum Details {
    /// Named fields, keyed by label.
    /// Renders as a JSON object.
    Fields(Vec<DetailRow>),

    /// An unnamed sequence.
    /// Renders as a JSON array of each item's [`DetailItem::json`], so a
    /// structured item survives the trip rather than collapsing to its display
    /// text.
    Items(Vec<DetailItem>),
}

impl Details {
    /// Whether the view carries nothing to render.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Fields(rows) => rows.is_empty(),
            Self::Items(items) => items.is_empty(),
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

/// Render a list table as a JSON array of objects.
///
/// Each row becomes an object keyed by the header cell content.
#[must_use]
#[expect(clippy::needless_pass_by_value)]
pub fn list_json(header: Row, rows: Vec<Row>) -> serde_json::Value {
    let headers: Vec<String> = header
        .cell_iter()
        .map(|c| strip_ansi_escapes::strip_str(c.content()))
        .collect();

    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (idx, cell) in row.cell_iter().enumerate() {
                let key = headers.get(idx).cloned().unwrap_or_else(|| idx.to_string());
                let val = strip_ansi_escapes::strip_str(cell.content());
                obj.insert(key, val.into());
            }
            serde_json::Value::Object(obj)
        })
        .collect();

    serde_json::Value::Array(items)
}

/// Render a details view as a borderless table.
#[must_use]
pub fn details(title: Option<&str>, body: Details) -> String {
    let mut buf = String::new();

    if let Some(title) = title {
        buf.push_str(title);
        if !body.is_empty() {
            buf.push_str("\n\n");
        }
    }

    let mut table = Table::new();
    table.load_preset(EMPTY);
    match body {
        Details::Fields(rows) => {
            for row in rows {
                table.add_row(detail_pretty_row(row));
            }
        }
        // A listing has no key column, so each item is a single cell.
        Details::Items(items) => {
            for item in items {
                let mut row = Row::new();
                row.add_cell(Cell::new(item.text).set_alignment(CellAlignment::Left));
                table.add_row(row);
            }
        }
    }
    buf.push_str(&table.trim_fmt());

    buf
}

/// Build a pretty (borderless table) row from a detail row.
///
/// A list value renders with the label on its own line and the items numbered
/// from 1 beneath it (the leading newline pushes the items below the label,
/// indented into the value column).
/// The numbers are right-aligned so the item text stays in one column past the
/// tenth item.
fn detail_pretty_row(row: DetailRow) -> Row {
    let value = match row.value {
        DetailValue::Scalar(s) => s,
        DetailValue::List(items) => {
            let width = items.len().to_string().len();
            let numbered = items
                .into_iter()
                .enumerate()
                .map(|(index, item)| format!("{:>width$}. {}", index + 1, item.text))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n{numbered}")
        }
    };

    let mut r = Row::new();
    r.add_cell(Cell::new(row.label).set_alignment(CellAlignment::Right));
    r.add_cell(Cell::new(value).set_alignment(CellAlignment::Left));
    r
}

/// Render a key-value details table as a pipe-delimited markdown table.
#[must_use]
pub fn details_markdown(title: Option<&str>, body: Details) -> String {
    let mut buf = String::new();

    if let Some(title) = title {
        buf.push_str(title);
        if !body.is_empty() {
            buf.push('\n');
        }
    }

    if body.is_empty() {
        return buf;
    }

    let md_rows = detail_markdown_rows(body);
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
fn detail_markdown_rows(body: Details) -> Vec<Row> {
    let mut out = Vec::new();
    match body {
        Details::Fields(rows) => {
            for row in rows {
                match row.value {
                    DetailValue::Scalar(s) => out.push(md_row(Some(&row.label), &s)),
                    DetailValue::List(items) => {
                        for (idx, item) in items.iter().enumerate() {
                            let label = if idx == 0 { row.label.as_str() } else { "" };
                            out.push(md_row(Some(label), &item.text));
                        }
                    }
                }
            }
        }
        // A listing has no key column.
        Details::Items(items) => {
            for item in items {
                out.push(md_row(None, &item.text));
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

/// Render a details view as JSON.
///
/// The [`Details`] variant fixes the shape of `details`, so it is the same for
/// every invocation of a given command: [`Details::Fields`] is an object keyed
/// by label, [`Details::Items`] is an array.
/// An empty view keeps its variant's shape, so a consumer never meets `{}`
/// where it expected `[]`.
///
/// A field's value keeps its own form: a scalar is a string, a list is an array
/// of each item's [`DetailItem::json`], so structure survives the trip rather
/// than collapsing to display text.
#[must_use]
pub fn details_json(title: Option<&str>, body: Details) -> serde_json::Value {
    let details = match body {
        Details::Fields(rows) => {
            let mut map = serde_json::Map::new();
            for DetailRow { label, value } in rows {
                map.insert(strip(&label), detail_json_value(value));
            }
            serde_json::Value::Object(map)
        }
        Details::Items(items) => {
            serde_json::Value::Array(items.into_iter().map(|item| item.json).collect())
        }
    };

    serde_json::json!({
        "title": title,
        "details": details,
    })
}

fn detail_json_value(value: DetailValue) -> serde_json::Value {
    match value {
        DetailValue::Scalar(s) => strip(&s).into(),
        DetailValue::List(items) => items
            .into_iter()
            .map(|item| item.json)
            .collect::<Vec<_>>()
            .into(),
    }
}

fn strip(s: &str) -> String {
    strip_ansi_escapes::strip_str(s)
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
