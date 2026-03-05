use comfy_table::{Cell, Row};
use jp_printer::{OutputFormat, Printer};
use serde_json::json;

use super::*;

fn row(cells: &[&str]) -> Row {
    let mut r = Row::new();
    for c in cells {
        r.add_cell(Cell::new(c));
    }
    r
}

fn flush_stdout(printer: &Printer, out: &jp_printer::SharedBuffer) -> String {
    printer.flush();
    out.lock().clone()
}

#[test]
fn table_text_pretty_renders_unicode_box() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);
    let header = row(&["Name", "Age"]);
    let rows = vec![row(&["Alice", "30"]), row(&["Bob", "25"])];

    print_table(&printer, header, rows);
    let output = flush_stdout(&printer, &out);

    // Unicode box-drawing uses these characters
    assert!(output.contains('─'), "expected box-drawing chars");
    assert!(output.contains("Alice"));
    assert!(output.contains("Bob"));
}

#[test]
fn table_text_renders_markdown_pipes() {
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let header = row(&["Name", "Age"]);
    let rows = vec![row(&["Alice", "30"])];

    print_table(&printer, header, rows);
    let output = flush_stdout(&printer, &out);

    assert!(output.contains("| Name"), "expected markdown table header");
    assert!(output.contains("| Alice"));
    assert!(output.contains("---"), "expected separator row");
}

#[test]
fn table_json_returns_compact_array() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);
    let header = row(&["Name", "Age"]);
    let rows = vec![row(&["Alice", "30"])];

    print_table(&printer, header, rows);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed, json!([{"Name": "Alice", "Age": "30"}]));
    // Compact: no interior newlines
    assert!(!output.trim().contains('\n'), "expected single-line JSON");
}

#[test]
fn table_json_pretty_returns_indented_array() {
    let (printer, out, _) = Printer::memory(OutputFormat::JsonPretty);
    let header = row(&["Id"]);
    let rows = vec![row(&["abc"])];

    print_table(&printer, header, rows);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed, json!([{"Id": "abc"}]));
    assert!(output.contains("\n  "), "expected indented JSON");
}

#[test]
fn table_empty_rows() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);
    let header = row(&["X"]);

    print_table(&printer, header, vec![]);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed, json!([]));
}

#[test]
fn details_text_pretty_with_title() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);
    let rows = vec![row(&["Key", "Value"])];

    print_details(&printer, Some("My Title"), rows);
    let output = flush_stdout(&printer, &out);

    assert!(output.contains("My Title"));
    assert!(output.contains("Key"));
    assert!(output.contains("Value"));
}

#[test]
fn details_text_renders_markdown() {
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let rows = vec![row(&["color", "red"])];

    print_details(&printer, None, rows);
    let output = flush_stdout(&printer, &out);

    assert!(output.contains('|'), "expected pipe-delimited output");
    assert!(output.contains("color"));
    assert!(output.contains("red"));
}

#[test]
fn details_json_compact() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);
    let rows = vec![row(&["name", "jp"]), row(&["version", "1.0"])];

    print_details(&printer, Some("info"), rows);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed["title"], "info");
    assert_eq!(parsed["details"]["name"], "jp");
    assert_eq!(parsed["details"]["version"], "1.0");
    assert!(!output.trim().contains('\n'), "expected compact JSON");
}

#[test]
fn details_json_pretty_is_indented() {
    let (printer, out, _) = Printer::memory(OutputFormat::JsonPretty);
    let rows = vec![row(&["k", "v"])];

    print_details(&printer, None, rows);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed["details"]["k"], "v");
    assert!(output.contains("\n  "), "expected indented JSON");
}

#[test]
fn details_no_title_json() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);
    let rows = vec![row(&["a", "b"])];

    print_details(&printer, None, rows);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert!(parsed["title"].is_null());
}

#[test]
fn json_value_compact() {
    let (printer, out, _) = Printer::memory(OutputFormat::Json);
    let value = json!({"key": "val", "num": 42});

    print_json(&printer, &value);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed, value);
    assert!(!output.trim().contains('\n'), "expected compact JSON");
}

#[test]
fn json_value_pretty_for_text_format() {
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);
    let value = json!({"a": 1});

    print_json(&printer, &value);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed, value);
    assert!(output.contains('\n'), "expected multi-line pretty output");
}

#[test]
fn json_value_pretty_for_json_pretty_format() {
    let (printer, out, _) = Printer::memory(OutputFormat::JsonPretty);
    let value = json!({"nested": {"x": true}});

    print_json(&printer, &value);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed, value);
    assert!(output.contains("\n  "), "expected indented JSON");
}

#[test]
fn json_value_plain_text_is_pretty() {
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let value = json!({"items": [1, 2, 3]});

    print_json(&printer, &value);
    let output = flush_stdout(&printer, &out);

    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed, value);
    // Text formats use to_string_pretty
    assert!(output.contains('\n'), "expected multi-line pretty output");
}
