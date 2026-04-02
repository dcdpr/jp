//! Backward-compatible deserialization for config deltas.
//!
//! When the `AppConfig` schema evolves (fields added, removed, or renamed), old
//! conversation streams may contain `ConfigDelta` events referencing fields
//! that no longer exist. The standard serde `deny_unknown_fields` on
//! `Partial*Config` types causes deserialization to fail entirely.
//!
//! This module provides schema-aware stripping: before deserializing a config
//! delta, we walk the JSON value alongside the current `AppConfig` schema and
//! remove any keys that don't exist in the schema. If deserialization still
//! fails after stripping (e.g. a field's type changed), we fall back to an
//! empty delta preserving only the timestamp.

use chrono::Utc;
use jp_config::{AppConfig, PartialAppConfig, Schema, SchemaType};
use serde_json::Value;
use tracing::warn;

use crate::{parse_dt, stream::ConfigDelta};

/// Deserialize a `ConfigDelta` from a raw JSON value, tolerating schema
/// changes.
///
/// 1. Strips unknown fields from the `delta` subtree using the current
///    `AppConfig` schema.
/// 2. Attempts typed deserialization.
/// 3. If that fails (e.g. a type changed), falls back to an empty delta with
///    just the timestamp preserved.
pub fn deserialize_config_delta(mut value: Value) -> ConfigDelta {
    let schema = AppConfig::schema();

    if let Some(delta) = value.get_mut("delta") {
        let stripped = strip_unknown_fields(delta, &schema);
        if stripped > 0 {
            warn!(
                count = stripped,
                "Stripped unknown fields from stored config delta.",
            );
        }
    }

    match serde_json::from_value::<ConfigDelta>(value.clone()) {
        Ok(delta) => delta,
        Err(err) => {
            warn!(
                error = %err,
                "Config delta incompatible with current schema, replacing with empty delta.",
            );
            fallback_config_delta(&value)
        }
    }
}

/// Extract just the timestamp from the raw JSON and build an empty delta.
fn fallback_config_delta(value: &Value) -> ConfigDelta {
    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|s| parse_dt(s).ok())
        .unwrap_or_else(Utc::now);

    ConfigDelta {
        timestamp,
        delta: Box::new(PartialAppConfig::empty()),
    }
}

/// Recursively strip JSON object keys that don't exist in the schema.
///
/// At each [`SchemaType::Struct`] level, retains only keys present in the
/// schema's field map and recurses into nested struct fields. Non-struct values
/// (leaves, arrays, enums) are left untouched.
///
/// Structs with any [`flatten`]ed field are skipped for stripping, because the
/// flattened field's entries appear as sibling keys that aren't in the schema's
/// explicit field map (e.g. per-tool overrides in `ToolsConfig`).
///
/// Returns the number of fields stripped.
///
/// [`flatten`]: jp_config::schema::SchemaField::flatten
fn strip_unknown_fields(value: &mut Value, schema: &Schema) -> usize {
    let SchemaType::Struct(ref struct_type) = schema.ty else {
        return 0;
    };

    let Some(obj) = value.as_object_mut() else {
        return 0;
    };

    let has_flatten = struct_type.fields.values().any(|f| f.flatten);

    let mut stripped = if has_flatten {
        0
    } else {
        let before = obj.len();
        obj.retain(|key, _| struct_type.fields.contains_key(key));
        before - obj.len()
    };

    // Recurse into known (non-flattened) struct fields.
    for (key, field) in &struct_type.fields {
        if field.flatten {
            continue;
        }

        let Some(child) = obj.get_mut(key) else {
            continue;
        };

        stripped += strip_unknown_fields(child, &field.schema);
    }

    stripped
}

#[cfg(test)]
#[path = "compat_tests.rs"]
mod tests;
