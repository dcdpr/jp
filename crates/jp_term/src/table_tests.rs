use comfy_table::Cell;

use super::*;

fn header() -> Row {
    let mut row = Row::new();
    row.add_cell(Cell::new("Name"));
    row.add_cell(Cell::new("Age"));
    row
}

fn rows() -> Vec<Row> {
    let mut r1 = Row::new();
    r1.add_cell(Cell::new("Alice"));
    r1.add_cell(Cell::new("30"));
    let mut r2 = Row::new();
    r2.add_cell(Cell::new("Bob"));
    r2.add_cell(Cell::new("7"));
    vec![r1, r2]
}

#[test]
fn markdown_list_table() {
    let output = list_markdown(header(), rows());
    assert_eq!(
        output,
        "| Name  | Age |
| ----- | --- |
| Alice | 30  |
| Bob   | 7   |
"
    );
}

#[test]
fn markdown_details_with_title() {
    let mut r1 = Row::new();
    r1.add_cell(Cell::new("key1"));
    r1.add_cell(Cell::new("val1"));
    let mut r2 = Row::new();
    r2.add_cell(Cell::new("longer-key"));
    r2.add_cell(Cell::new("v2"));

    let output = details_markdown(Some("Info"), vec![r1, r2]);
    assert_eq!(
        output,
        "Info
| key1       | val1 |
| longer-key | v2   |
"
    );
}

#[test]
fn markdown_details_no_title() {
    let mut r = Row::new();
    r.add_cell(Cell::new("a"));
    r.add_cell(Cell::new("b"));

    let output = details_markdown(None, vec![r]);
    assert_eq!(
        output,
        "| a | b |
"
    );
}

#[test]
fn json_list() {
    let json = list_json(header(), rows());
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["Name"], "Alice");
    assert_eq!(arr[0]["Age"], "30");
    assert_eq!(arr[1]["Name"], "Bob");
    assert_eq!(arr[1]["Age"], "7");
}

#[test]
fn json_details() {
    let mut r = Row::new();
    r.add_cell(Cell::new("ID"));
    r.add_cell(Cell::new("jp-c123"));

    let json = details_json(Some("title"), vec![r]);
    assert_eq!(json["title"], "title");
    assert_eq!(json["details"]["ID"], "jp-c123");
}

#[test]
fn json_details_strips_ansi() {
    let mut r = Row::new();
    r.add_cell(Cell::new("\x1b[1mKey\x1b[0m"));
    r.add_cell(Cell::new("\x1b[32mVal\x1b[0m"));

    let json = details_json(None, vec![r]);
    assert_eq!(json["details"]["Key"], "Val");
}

#[test]
fn markdown_strips_ansi() {
    let mut h = Row::new();
    h.add_cell(Cell::new("\x1b[1mBold\x1b[0m"));
    let mut r = Row::new();
    r.add_cell(Cell::new("\x1b[32mGreen\x1b[0m"));

    let output = list_markdown(h, vec![r]);
    // Column widths should be based on visual width, not byte count.
    assert!(output.contains("| Bold  |"), "got: {output}");
    assert!(output.contains("| Green |"), "got: {output}");
}
