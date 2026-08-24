//! Format-aware output helpers.
//!
//! These functions dispatch table/details/value rendering based on the
//! printer's [`OutputFormat`], so commands don't need to branch on the format
//! themselves.
//!
//! A command whose two views carry the same thing lets the JSON be derived from
//! the display rows: [`Details`] fixes the shape, and each [`DetailItem`]
//! carries its own structured form.
//!
//! When the views diverge, the command supplies the payload instead
//! ([`print_table`], or an `is_json()` branch onto [`print_json`]).
//! That decouples the two: display labels, ordering, and layout can change
//! freely, while the payload stays a stable machine contract with `snake_case`
//! keys.
//!
//! [`DetailItem`]: jp_term::table::DetailItem

use comfy_table::Row;
use jp_printer::{OutputFormat, Printer};
use jp_term::table::{Details, details, details_json, details_markdown, list, list_markdown};
use serde_json::{Value, to_string, to_string_pretty};

/// Print a list table (header + rows) in the format dictated by the printer.
///
/// - `TextPretty` → unicode box-drawing table
/// - `Text` → pipe-delimited markdown table
/// - `Json` / `JsonPretty` → the explicit `json` payload
pub fn print_table(printer: &Printer, header: Row, rows: Vec<Row>, footer: bool, json: &Value) {
    let output = match printer.format() {
        OutputFormat::TextPretty => list(header, rows, footer),
        OutputFormat::Text => list_markdown(header, rows),
        OutputFormat::Json => to_string(json).unwrap_or_else(|_| json.to_string()),
        OutputFormat::JsonPretty => to_string_pretty(json).unwrap_or_else(|_| json.to_string()),
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
pub fn print_details(printer: &Printer, title: Option<&str>, body: Details) {
    let output = match printer.format() {
        OutputFormat::TextPretty => details(title, body),
        OutputFormat::Text => details_markdown(title, body),
        OutputFormat::Json => {
            let json = details_json(title, body);
            to_string(&json).unwrap_or_else(|_| json.to_string())
        }
        OutputFormat::JsonPretty => {
            let json = details_json(title, body);
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
