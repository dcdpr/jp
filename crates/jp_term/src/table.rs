use comfy_table::{Cell, Row, Table};

pub const EMPTY: &str = "                   ";
pub const UTF8_FULL: &str = "││──├──┤     ──╭╮╰╯";

#[must_use]
pub fn list(header: Row, rows: Vec<Row>) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(header);
    table.add_rows(rows);

    table.trim_fmt()
}

#[must_use]
pub fn list_json(_headers: Row, _rows: Vec<Row>) -> serde_json::Value {
    serde_json::json!({})
}

#[must_use]
pub fn details(title: Option<&str>, rows: Vec<Row>) -> String {
    let mut buf = String::new();

    if let Some(title) = title {
        buf.push_str(title);
        buf.push_str("\n\n");
    }

    let mut table = Table::new();
    table.load_preset(EMPTY);
    table.add_rows(rows);
    buf.push_str(&table.trim_fmt());

    buf
}

#[must_use]
pub fn details_json(title: Option<&str>, rows: Vec<Row>) -> serde_json::Value {
    let mut details = serde_json::Map::new();
    for row in rows {
        let mut iter = row.cell_iter();
        let Some(value) = iter.next().map(Cell::content) else {
            continue;
        };

        let key = iter.next().map(Cell::content).unwrap_or_default();

        details.insert(key, value.into());
    }

    serde_json::json!({
        "title": title,
        "details": details,
    })
}
