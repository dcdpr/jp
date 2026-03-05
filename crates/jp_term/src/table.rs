use comfy_table::{Row, Table};

pub const EMPTY: &str = "                   ";
pub const UTF8_FULL: &str = "││──├──┤     ──╭╮╰╯";

/// Render a list table with unicode box-drawing characters.
#[must_use]
pub fn list(header: Row, rows: Vec<Row>) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(header);
    table.add_rows(rows);

    table.trim_fmt()
}

/// Render a list table as a pipe-delimited markdown table.
///
/// Produces output like:
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

/// Render a key-value details table with no borders.
#[must_use]
pub fn details(title: Option<&str>, rows: Vec<Row>) -> String {
    let mut buf = String::new();

    if let Some(title) = title {
        buf.push_str(title);
        if !rows.is_empty() {
            buf.push_str("\n\n");
        }
    }

    let mut table = Table::new();
    table.load_preset(EMPTY);
    table.add_rows(rows);
    buf.push_str(&table.trim_fmt());

    buf
}

/// Render a key-value details table as a pipe-delimited markdown table.
#[must_use]
#[expect(clippy::needless_pass_by_value)]
pub fn details_markdown(title: Option<&str>, rows: Vec<Row>) -> String {
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

    let row_refs: Vec<&Row> = rows.iter().collect();
    let col_count = max_columns(&row_refs);
    let widths = column_widths(&row_refs, col_count);

    for row in &rows {
        push_md_row(&mut buf, row, &widths, col_count);
    }

    buf
}

/// Render key-value details as JSON.
#[must_use]
pub fn details_json(title: Option<&str>, rows: Vec<Row>) -> serde_json::Value {
    let mut details = serde_json::Map::new();
    for row in rows {
        let mut iter = row.cell_iter();
        let Some(key) = iter
            .next()
            .map(|c| strip_ansi_escapes::strip_str(c.content()))
        else {
            continue;
        };

        let value = iter
            .next()
            .map(|c| strip_ansi_escapes::strip_str(c.content()))
            .unwrap_or_default();

        details.insert(key, value.into());
    }

    serde_json::json!({
        "title": title,
        "details": details,
    })
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
