//! Conversation-specific types and utilities.

#![warn(
    clippy::all,
    clippy::allow_attributes,
    clippy::cargo,
    clippy::missing_docs_in_private_items,
    clippy::nursery,
    clippy::pedantic,
    clippy::renamed_function_params,
    clippy::tests_outside_test_module,
    clippy::todo,
    clippy::try_err,
    clippy::unimplemented,
    clippy::unneeded_field_pattern,
    clippy::unseparated_literal_suffix,
    clippy::unused_result_ok,
    clippy::unused_trait_names,
    clippy::use_debug,
    clippy::unwrap_used,
    missing_docs,
    rustdoc::all,
    unused_doc_comments
)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "we need to update rmcp to update base64"
)]

pub mod conversation;
pub mod error;
pub mod event;
pub mod stream;
pub mod thread;

pub use conversation::{Conversation, ConversationId, ConversationsMetadata};
pub use error::Error;
pub use event::{ConversationEvent, EventKind};
pub use stream::{ConversationStream, StreamError};

/// Format `DateTime<Utc>` like `time`'s human-readable serde: `"2023-01-01 00:00:00.0"`.
fn fmt_dt(dt: &chrono::DateTime<chrono::Utc>) -> String {
    if dt.timestamp_subsec_nanos() == 0 {
        return dt.format("%Y-%m-%d %H:%M:%S.0").to_string();
    }
    dt.format("%Y-%m-%d %H:%M:%S.%9f")
        .to_string()
        .trim_end_matches('0')
        .to_owned()
}

/// Parse from `time`'s format or RFC 3339.
fn parse_dt(s: &str) -> Result<chrono::DateTime<chrono::Utc>, String> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .map(|dt| dt.and_utc())
        .or_else(|_| {
            chrono::DateTime::parse_from_rfc3339(s).map(|dt| dt.with_timezone(&chrono::Utc))
        })
        .map_err(|e| e.to_string())
}

pub(crate) fn serialize_dt<S: serde::Serializer>(
    dt: &chrono::DateTime<chrono::Utc>,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&fmt_dt(dt))
}

pub(crate) fn deserialize_dt<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<chrono::DateTime<chrono::Utc>, D::Error> {
    let s = <String as serde::Deserialize>::deserialize(d)?;
    parse_dt(&s).map_err(serde::de::Error::custom)
}

pub(crate) fn serialize_dt_opt<S: serde::Serializer>(
    dt: &Option<chrono::DateTime<chrono::Utc>>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match dt {
        Some(dt) => s.serialize_some(&fmt_dt(dt)),
        None => s.serialize_none(),
    }
}

pub(crate) fn deserialize_dt_opt<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, D::Error> {
    let s: Option<String> = serde::Deserialize::deserialize(d)?;
    match s {
        Some(s) => parse_dt(&s).map(Some).map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}
