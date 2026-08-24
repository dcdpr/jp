use comfy_table::Cell;

use super::*;

/// The pretty details view of a two-item list, exactly as it reaches the
/// terminal: the label on the first line, items numbered and indented into the
/// value column.
const PRETTY_LIST_TWO_ITEMS: &str = " Attachments
              1. a://x
              2. b://y";

/// The same view for ten items, where the single-digit numbers are padded to
/// line up with `10.`.
const PRETTY_LIST_TEN_ITEMS: &str = " Items
         1. item-1
         2. item-2
         3. item-3
         4. item-4
         5. item-5
         6. item-6
         7. item-7
         8. item-8
         9. item-9
        10. item-10";

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

fn row(cells: &[&str]) -> Row {
    let mut r = Row::new();
    for c in cells {
        r.add_cell(Cell::new(c));
    }
    r
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
    let output = details_markdown(
        Some("Info"),
        Details::Fields(vec![
            DetailRow::scalar("key1", "val1"),
            DetailRow::scalar("longer-key", "v2"),
        ]),
    );
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
    let output = details_markdown(None, Details::Fields(vec![DetailRow::scalar("a", "b")]));
    assert_eq!(
        output,
        "| a | b |
"
    );
}

#[test]
fn pretty_details_list_puts_label_above_numbered_items() {
    let output = details(
        None,
        Details::Fields(vec![DetailRow::list("Attachments", vec![
            DetailItem::plain("a://x"),
            DetailItem::plain("b://y"),
        ])]),
    );

    assert_eq!(output, PRETTY_LIST_TWO_ITEMS);
}

#[test]
fn pretty_details_list_right_aligns_numbers_past_the_tenth_item() {
    // Single-digit numbers are padded so every item's text starts in the same
    // column.
    let items = (1..=10)
        .map(|n| DetailItem::plain(format!("item-{n}")))
        .collect();
    let output = details(None, Details::Fields(vec![DetailRow::list("Items", items)]));

    assert_eq!(output, PRETTY_LIST_TEN_ITEMS);
}

#[test]
fn markdown_details_list_expands_to_one_row_per_item() {
    let output = details_markdown(
        None,
        Details::Fields(vec![DetailRow::list("Attachments", vec![
            DetailItem::plain("a://x"),
            DetailItem::plain("b://y"),
        ])]),
    );

    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "got: {output}");
    assert!(lines[0].contains("Attachments"), "got: {output}");
    assert!(lines[0].contains("a://x"), "got: {output}");
    // Continuation row carries a blank label, not a repeated one.
    assert!(!lines[1].contains("Attachments"), "got: {output}");
    assert!(lines[1].contains("b://y"), "got: {output}");
}

#[test]
fn json_details_list_of_plain_items_is_string_array() {
    let json = details_json(
        None,
        Details::Fields(vec![DetailRow::list("Attachments", vec![
            DetailItem::plain("a://x"),
            DetailItem::plain("b://y"),
        ])]),
    );

    assert_eq!(
        json["details"]["Attachments"],
        serde_json::json!(["a://x", "b://y"])
    );
}

#[test]
fn list_item_text_and_json_forms_can_differ() {
    let item = DetailItem::new(
        "cmd (Desc): cmd://x",
        serde_json::json!({ "scheme": "cmd", "url": "cmd://x" }),
    );
    let rows = Details::Fields(vec![DetailRow::list("Attachments", vec![item])]);

    // Pretty uses the text form.
    assert!(
        details(None, rows.clone()).contains("1. cmd (Desc): cmd://x"),
        "text form should drive the pretty view"
    );

    // JSON uses the structured form.
    let json = details_json(None, rows);
    assert_eq!(json["details"]["Attachments"][0]["scheme"], "cmd");
    assert_eq!(json["details"]["Attachments"][0]["url"], "cmd://x");
}

/// A listing has no keys, so it renders as an array of each item's structured
/// form rather than collapsing to display text or inventing keys from values.
#[test]
fn json_details_items_are_an_array() {
    let items = vec![
        DetailItem::new("foo", serde_json::json!({ "key": "foo", "value": "" })),
        DetailItem::new(
            "qux=quux",
            serde_json::json!({ "key": "qux", "value": "quux" }),
        ),
    ];

    let json = details_json(Some("jp-c123"), Details::Items(items));

    assert_eq!(json["title"], "jp-c123");
    assert_eq!(
        json["details"],
        serde_json::json!([
            { "key": "foo", "value": "" },
            { "key": "qux", "value": "quux" },
        ])
    );
}

/// A listing renders one cell per item, with no key column.
#[test]
fn pretty_details_items_have_no_key_column() {
    let output = details(
        Some("Attachments"),
        Details::Items(vec![DetailItem::plain("a://x"), DetailItem::plain("b://y")]),
    );

    assert_eq!(output, "Attachments\n\n a://x\n b://y");
}

/// The variant fixes the shape, so it is the same whether or not a given
/// invocation had anything to show.
/// A consumer never meets `{}` where it expected `[]`, which is what a rule
/// keyed on the data would produce.
#[test]
fn json_details_shape_follows_the_variant_not_the_data() {
    assert_eq!(
        details_json(None, Details::Fields(vec![]))["details"],
        serde_json::json!({}),
        "an empty record is still an object"
    );
    assert_eq!(
        details_json(None, Details::Fields(vec![DetailRow::scalar("ID", "x")]))["details"],
        serde_json::json!({ "ID": "x" })
    );

    assert_eq!(
        details_json(None, Details::Items(vec![]))["details"],
        serde_json::json!([]),
        "an empty listing is still an array"
    );
    assert_eq!(
        details_json(None, Details::Items(vec![DetailItem::plain("a://x")]))["details"],
        serde_json::json!(["a://x"])
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
    let json = details_json(
        Some("title"),
        Details::Fields(vec![DetailRow::scalar("ID", "jp-c123")]),
    );
    assert_eq!(json["title"], "title");
    assert_eq!(json["details"]["ID"], "jp-c123");
}

#[test]
fn json_details_strips_ansi() {
    let json = details_json(
        None,
        Details::Fields(vec![DetailRow::scalar(
            "\x1b[1mKey\x1b[0m",
            "\x1b[32mVal\x1b[0m",
        )]),
    );
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

#[test]
fn list_without_footer() {
    let output = list(header(), rows(), false);
    let lines: Vec<&str> = output.lines().collect();
    // top border, header, separator, 2 data rows, bottom border = 6
    assert_eq!(lines.len(), 6);
    assert!(lines[0].starts_with('\u{256d}'), "expected top border");
    assert!(lines[5].starts_with('\u{2570}'), "expected bottom border");
}

#[test]
fn list_with_footer() {
    let data: Vec<Row> = (0..5).map(|i| row(&[&format!("name-{i}"), "99"])).collect();
    let output = list(header(), data, true);
    let lines: Vec<&str> = output.lines().collect();

    // 5 data rows + top border + header + separator + footer separator + footer header + bottom border = 11
    assert_eq!(lines.len(), 11, "got:\n{output}");

    let header_line = lines[1];
    let separator = lines[2];
    let footer_sep = lines[lines.len() - 3];
    let footer_header = lines[lines.len() - 2];
    let bottom = lines[lines.len() - 1];

    assert_eq!(
        footer_sep, separator,
        "footer separator should match header separator"
    );
    assert_eq!(
        footer_header, header_line,
        "footer header should match header"
    );
    assert!(bottom.starts_with('\u{2570}'), "expected bottom border");
}

#[test]
fn list_footer_skipped_for_single_data_row() {
    // A single data row produces 5 lines (top, header, sep, row, bottom),
    // which is the minimum; the guard skips the footer.
    let output_with = list(header(), vec![row(&["Alice", "30"])], true);
    let output_without = list(header(), vec![row(&["Alice", "30"])], false);
    assert_eq!(output_with, output_without);
}
