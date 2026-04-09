//! Format-aware output helpers.
//!
//! These functions dispatch table/details/value rendering based on the
//! printer's [`OutputFormat`], so commands don't need to branch on the format
//! themselves.

use comfy_table::Row;
use jp_printer::{OutputFormat, Printer};
use jp_term::table::{details, details_json, details_markdown, list, list_json, list_markdown};
use serde_json::{Value, to_string, to_string_pretty};

/// Print a list table (header + rows) in the format dictated by the printer.
///
/// - `TextPretty` → unicode box-drawing table
/// - `Text` → pipe-delimited markdown table
/// - `Json` / `JsonPretty` → JSON array of objects
pub fn print_table(printer: &Printer, header: Row, rows: Vec<Row>, footer: bool) {
    let output = match printer.format() {
        OutputFormat::TextPretty => list(header, rows, footer),
        OutputFormat::Text => list_markdown(header, rows),
        OutputFormat::Json => {
            let json = list_json(header, rows);
            to_string(&json).unwrap_or_else(|_| json.to_string())
        }
        OutputFormat::JsonPretty => {
            let json = list_json(header, rows);
            to_string_pretty(&json).unwrap_or_else(|_| json.to_string())
        }
    };

    // Use println_raw: JSON variants already contain valid JSON, text variants
    // should not be wrapped in a JSON envelope either.
    printer.println_raw(&output);
}

/// Print a key-value details view in the format dictated by the printer.
///
/// - `TextPretty` → borderless aligned table with optional title
/// - `Text` → pipe-delimited markdown table with optional title
/// - `Json` / `JsonPretty` → JSON object
pub fn print_details(printer: &Printer, title: Option<&str>, rows: Vec<Row>) {
    let output = match printer.format() {
        OutputFormat::TextPretty => details(title, rows),
        OutputFormat::Text => details_markdown(title, rows),
        OutputFormat::Json => {
            let json = details_json(title, rows);
            to_string(&json).unwrap_or_else(|_| json.to_string())
        }
        OutputFormat::JsonPretty => {
            let json = details_json(title, rows);
            to_string_pretty(&json).unwrap_or_else(|_| json.to_string())
        }
    };

    printer.println_raw(&output);
}

/// Print a JSON value in the format dictated by the printer.
///
/// - Text formats → `serde_json::to_string_pretty`
/// - `Json` → compact JSON
/// - `JsonPretty` → indented JSON
pub fn print_json(printer: &Printer, value: &Value) {
    let output = match printer.format() {
        OutputFormat::Json => to_string(value).unwrap_or_else(|_| value.to_string()),
        _ => to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    };

    printer.println_raw(&output);
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
