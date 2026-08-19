//! Base64 encoding/decoding for storage-persisted event fields.
//!
//! This module encodes select content fields (tool arguments, tool response
//! content, metadata) so that raw conversation text doesn't appear in plain
//! text on disk - keeping it out of `grep` and editor search results.
//!
//! The encoding is applied during `InternalEvent` serialization and reversed
//! during deserialization.
//! The inner event types serialize as plain text.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};

use crate::{event::EventKind, parse_dt};

/// A timestamp in the one format events cross a boundary in.
///
/// Storage keeps timestamps in `time`'s human-readable format (`2024-09-01
/// 10:00:00.0`), which nothing outside Rust parses.
/// A reader on the far side of a boundary wants one format, and RFC 3339 is the
/// one every platform's date parser accepts.
///
/// Sub-second precision is kept when the value has any and omitted when it does
/// not, which is what `AutoSi` means: a timestamp stored with a fractional part
/// keeps it.
#[must_use]
pub fn rfc3339(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// A stored timestamp, re-spelled as RFC 3339.
///
/// `None` for a value that parses as neither storage's format nor RFC 3339,
/// which a caller should leave as it found it: a timestamp nobody can read is
/// still better than no field at all.
///
/// For a caller holding raw JSON rather than a typed event — an entry written
/// by a newer build, kept verbatim, whose timestamp still has to reach a reader
/// in the same format as every other one.
/// A caller holding the typed value calls [`rfc3339`] and parses nothing.
#[must_use]
pub fn rfc3339_str(timestamp: &str) -> Option<String> {
    parse_dt(timestamp).ok().map(rfc3339)
}

/// Which encoding to apply to a given field.
enum Field {
    /// Base64-encode the string value itself.
    String(&'static str),

    /// Base64-encode all string values within a JSON map, recursively.
    Map(&'static str),
}

impl Field {
    /// Encode the field in the given value.
    fn encode(&self, value: &mut Map<String, Value>) {
        match self {
            Self::String(key) => {
                if let Some(v) = value.get_mut(*key) {
                    encode_string(v);
                }
            }
            Self::Map(key) => {
                if let Some(v) = value.get_mut(*key) {
                    encode_map_strings(v);
                }
            }
        }
    }
}

/// Encode content fields for a conversation event being written to storage.
///
/// The `kind` is used to select the correct field mapping via an exhaustive
/// match — adding a new [`EventKind`] variant will produce a compiler error
/// here, forcing the developer to decide which fields (if any) need encoding.
pub fn encode_event(value: &mut Value, kind: &EventKind) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    // Metadata is present on all events.
    if let Some(v) = obj.get_mut("metadata") {
        encode_map_strings(v);
    }

    // Each variant is listed explicitly so adding a new EventKind forces a
    // decision about which fields (if any) need encoding.
    //
    // NOTE: don't forget to update `decode_event_value` when adding new
    // variants!
    let fields: &[Field] = match kind {
        EventKind::ToolCallRequest(_) => &[Field::Map("arguments")],
        EventKind::ToolCallResponse(_) => &[Field::String("content")],
        EventKind::TurnStart(_)
        | EventKind::ChatRequest(_)
        | EventKind::ChatResponse(_)
        | EventKind::InquiryRequest(_)
        | EventKind::InquiryResponse(_) => &[],
    };

    for field in fields {
        field.encode(obj);
    }
}

/// Decode base64-encoded storage fields from a raw event JSON value.
///
/// This uses the `type` tag to determine which fields to decode, mirroring the
/// encoding in [`encode_event`].
/// Unknown event types (including `config_delta`) are silently ignored.
pub fn decode_event_value(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    // Metadata is present on all events.
    if let Some(v) = obj.get_mut("metadata") {
        decode_map_strings(v);
    }

    let tag = obj.get("type").and_then(Value::as_str).unwrap_or_default();

    match tag {
        "tool_call_request" => {
            if let Some(v) = obj.get_mut("arguments") {
                decode_map_strings(v);
            }
        }
        "tool_call_response" => {
            if let Some(v) = obj.get_mut("content") {
                decode_string(v);
            }
        }
        // All other event types (turn_start, chat_request, chat_response,
        // inquiry_*, config_delta, etc.) have no base64 fields.
        _ => {}
    }
}

/// Base64-encode a single JSON string value in place.
fn encode_string(value: &mut Value) {
    if let Value::String(s) = value {
        *s = STANDARD.encode(s.as_bytes());
    }
}

/// Decode a base64-encoded JSON string value in place.
/// Non-base64 strings are left untouched.
fn decode_string(value: &mut Value) {
    if let Value::String(s) = value
        && let Ok(bytes) = STANDARD.decode(s.as_bytes())
        && let Ok(decoded) = String::from_utf8(bytes)
    {
        *s = decoded;
    }
}

/// Recursively base64-encode all string values in a JSON tree.
fn encode_map_strings(value: &mut Value) {
    match value {
        Value::String(s) => *s = STANDARD.encode(s.as_bytes()),
        Value::Array(arr) => arr.iter_mut().for_each(encode_map_strings),
        Value::Object(obj) => obj.values_mut().for_each(encode_map_strings),
        _ => {}
    }
}

/// Recursively decode all base64-encoded string values in a JSON tree.
fn decode_map_strings(value: &mut Value) {
    match value {
        Value::String(s) => {
            if let Ok(bytes) = STANDARD.decode(s.as_bytes())
                && let Ok(decoded) = String::from_utf8(bytes)
            {
                *s = decoded;
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(decode_map_strings),
        Value::Object(obj) => obj.values_mut().for_each(decode_map_strings),
        _ => {}
    }
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
