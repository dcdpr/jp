//! Parser for `JP_DEBUG=1` JSON-per-line trace logs.
//!
//! Each line is a `tracing-subscriber::fmt::json()`-formatted event.
//! We keep parsing tolerant: a malformed line is skipped, not fatal, so a
//! single truncated trailing line doesn't lose the whole report.

use serde::Deserialize;
use serde_json::{Map, Value};

/// Text-format marker jp writes to stderr, right before exit, when `JP_DEBUG=1`
/// and the output format is human-readable text.
pub(crate) const TRACE_PATH_PREFIX: &str = "Full trace log written to: ";

/// Severity, ordered so `level >= Level::Info` is meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// Parse a case-insensitive level name.
    /// Accepts `WARNING` as an alias for `WARN` because some external trace
    /// producers emit it that way.
    #[must_use]
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "TRACE" => Some(Self::Trace),
            "DEBUG" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "WARN" | "WARNING" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            _ => None,
        }
    }

    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// One parsed event from a trace log line.
#[derive(Debug, Clone)]
pub(crate) struct TraceEvent {
    /// Raw RFC3339 timestamp as written by tracing-subscriber.
    pub timestamp: String,
    pub level: Level,
    pub target: String,
    /// Human-readable message (extracted out of `fields.message`).
    pub message: String,
    /// All structured fields other than `message`.
    /// Order is preserved by `serde_json`'s `preserve_order` feature.
    pub fields: Map<String, Value>,
    /// Span stack, root-first.
    /// Empty when the event was emitted outside a span.
    pub spans: Vec<String>,
}

/// Parse every line in `text`, skipping blanks and unparseable lines.
pub(crate) fn parse_lines(text: &str) -> Vec<TraceEvent> {
    text.lines().filter_map(parse_line).collect()
}

/// Extract the trace log path jp writes to stderr right before exit.
///
/// jp emits `Full trace log written to: <path>` when the output format is text,
/// or a `{"trace_log": "<path>"}` JSON object when the format is JSON or
/// JSON-pretty.
/// This checks each stderr line for either shape.
pub(crate) fn extract_trace_path(stderr: &str) -> Option<String> {
    stderr.lines().find_map(parse_trace_path_line)
}

/// True for the stderr line jp emits to announce the trace log path, in either
/// the text or JSON marker format.
pub(crate) fn is_trace_path_marker_line(line: &str) -> bool {
    parse_trace_path_line(line).is_some()
}

fn parse_trace_path_line(line: &str) -> Option<String> {
    if let Some(path) = line.strip_prefix(TRACE_PATH_PREFIX) {
        return Some(path.trim().to_owned());
    }
    match serde_json::from_str::<Value>(line).ok()? {
        Value::Object(obj) => match obj.get("trace_log")? {
            Value::String(path) => Some(path.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn parse_line(line: &str) -> Option<TraceEvent> {
    if line.trim().is_empty() {
        return None;
    }
    let raw: Raw = serde_json::from_str(line).ok()?;
    let level = Level::parse(&raw.level)?;

    let mut fields = raw.fields.unwrap_or_default();
    let message = fields
        .shift_remove("message")
        .map(|v| match v {
            Value::String(s) => s,
            other => other.to_string(),
        })
        .unwrap_or_default();

    let spans = raw
        .spans
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.name)
        .collect();

    Some(TraceEvent {
        timestamp: raw.timestamp,
        level,
        target: raw.target,
        message,
        fields,
        spans,
    })
}

#[derive(Deserialize)]
struct Raw {
    timestamp: String,
    level: String,
    target: String,
    #[serde(default)]
    fields: Option<Map<String, Value>>,
    #[serde(default)]
    spans: Option<Vec<RawSpan>>,
}

#[derive(Deserialize)]
struct RawSpan {
    name: String,
}

#[cfg(test)]
#[path = "trace_parse_tests.rs"]
mod tests;
