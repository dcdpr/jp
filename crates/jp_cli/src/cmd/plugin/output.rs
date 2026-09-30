//! A plugin's `print` messages, through the host's output rules.
//!
//! The channel decides the stream: `content` is data on stdout; `chrome`,
//! `tool_call`, `tool_result` and `reasoning` are chrome on stderr, which
//! `--quiet` silences; `error` goes to stderr and is never silenced.
//! In terminal output the format decides the rendering.
//! Under a JSON output format rendering is off: a `json` print is emitted as
//! the value its text holds, and anything else is wrapped in the printer's
//! message record.
//!
//! See: `docs/rfd/072-command-plugin-system.md`, "Output".

use jp_config::style::markdown::MarkdownConfig;
use jp_md::format::Formatter;
use jp_plugin::message::PrintMessage;
use jp_printer::{OutputFormat, PrintableExt as _, Printer};
use serde_json::Value;
use tracing::warn;

/// Where a print goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stream {
    /// Data, on stdout.
    Content,

    /// Chrome, on stderr, silenced by `--quiet`.
    Chrome,

    /// An error, on stderr, never silenced.
    Error,
}

/// What reaches the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Emit {
    /// Text for the printer to write, wrapped in a message record under a JSON
    /// output format.
    Text(String),

    /// A JSON value, written as a record of its own.
    Json(String),
}

/// The stream a channel names.
///
/// An unknown channel is treated as content, so a plugin built for a newer host
/// that adds channels still has its output shown.
pub(crate) fn stream(channel: &str) -> Stream {
    match channel {
        "content" => Stream::Content,
        "chrome" | "tool_call" | "tool_result" | "reasoning" => Stream::Chrome,
        "error" => Stream::Error,
        other => {
            warn!(
                channel = other,
                "Unknown plugin print channel; treating as content."
            );
            Stream::Content
        }
    }
}

/// Render a print for the host's output format.
pub(crate) fn render(print: &PrintMessage, format: OutputFormat, style: &MarkdownConfig) -> Emit {
    if format.is_json() {
        return render_json_mode(print, format);
    }

    let formatter = Formatter::with_width(style.wrap_width).theme(if format.is_pretty() {
        style.theme.as_deref()
    } else {
        None
    });

    Emit::Text(match print.format.as_str() {
        "plain" => print.text.clone(),
        "markdown" => formatter
            .format_terminal(&print.text)
            .unwrap_or_else(|_| print.text.clone()),
        "json" => match serde_json::from_str::<Value>(&print.text) {
            Ok(value) => {
                let pretty =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| print.text.clone());
                highlight(&formatter, &format!("{pretty}\n"), "json")
            }
            Err(error) => {
                warn!(%error, "Plugin printed invalid JSON; showing it as text.");
                print.text.clone()
            }
        },
        "code" => highlight(
            &formatter,
            &print.text,
            print.language.as_deref().unwrap_or(""),
        ),
        other => {
            warn!(
                format = other,
                "Unknown plugin print format; showing it as text."
            );
            print.text.clone()
        }
    })
}

/// Under a JSON output format, a `json` print is its own record; anything else
/// is text for the printer to wrap.
fn render_json_mode(print: &PrintMessage, format: OutputFormat) -> Emit {
    if print.format != "json" {
        return Emit::Text(print.text.clone());
    }

    match serde_json::from_str::<Value>(&print.text) {
        Ok(value) => {
            let json = if format.is_json_pretty() {
                serde_json::to_string_pretty(&value)
            } else {
                serde_json::to_string(&value)
            };
            Emit::Json(json.unwrap_or_else(|_| print.text.clone()))
        }
        Err(error) => {
            warn!(%error, "Plugin printed invalid JSON; wrapping it as a message.");
            Emit::Text(print.text.clone())
        }
    }
}

/// Syntax-highlight `text` as `language`, line by line.
fn highlight(formatter: &Formatter, text: &str, language: &str) -> String {
    let mut state = formatter.begin_code_block(language);

    text.split_inclusive('\n')
        .map(|line| formatter.render_code_line(line, &mut state, None, 0))
        .collect()
}

/// Write a plugin's print through the printer.
pub(crate) fn print(printer: &Printer, print: &PrintMessage, style: &MarkdownConfig) {
    let stream = stream(&print.channel);

    match (stream, render(print, printer.format(), style)) {
        (Stream::Content, Emit::Text(text)) => printer.print(text),
        (Stream::Chrome, Emit::Text(text)) => printer.eprint(text),
        (Stream::Content, Emit::Json(json)) => printer.println_raw(json),
        (Stream::Chrome, Emit::Json(json)) => printer.println_raw(json.to_err()),

        // An error's text is one report, so a trailing newline the plugin sent
        // is not a second, empty line.
        (Stream::Error, Emit::Text(text)) => {
            printer.error_println(text.trim_end_matches('\n'));
        }
        (Stream::Error, Emit::Json(json)) => printer.error_println_raw(json),
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
