//! Backward-compatible deserialization for [`PartialAppConfig`].
//!
//! When the [`AppConfig`] schema evolves (fields added, removed, or renamed),
//! old conversation data may reference fields that no longer exist. The
//! standard serde `deny_unknown_fields` on `Partial*Config` types causes
//! deserialization to fail entirely.
//!
//! This module provides schema-aware stripping: before deserializing, we walk
//! the JSON value alongside the current `AppConfig` schema and remove any keys
//! that don't exist in the schema. If deserialization still fails after
//! stripping (e.g. a field's type changed), we fall back to an empty config.

use jp_config::{AppConfig, PartialAppConfig, Schema, SchemaType};
use serde_json::Value;
use tracing::warn;

/// Deserialize a [`PartialAppConfig`] from a raw JSON value, tolerating
/// schema changes.
///
/// 1. Strips unknown fields using the current [`AppConfig`] schema.
/// 2. Attempts typed deserialization.
/// 3. If that fails (e.g. a field's type changed), falls back to
///    [`PartialAppConfig::empty()`].
///
/// Used for both the base config snapshot (`base_config.json`) and config delta
/// events in the event stream.
pub fn deserialize_partial_config(mut value: Value) -> PartialAppConfig {
    let schema = AppConfig::schema();

    let stripped = strip_unknown_fields(&mut value, &schema);
    if stripped > 0 {
        warn!(
            count = stripped,
            "Stripped unknown fields from stored config.",
        );
    }

    match serde_json::from_value::<PartialAppConfig>(value) {
        Ok(config) => config,
        Err(err) => {
            warn!(
                error = %err,
                "Stored config incompatible with current schema, replacing with empty config.",
            );
            PartialAppConfig::empty()
        }
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
    let SchemaType::Struct(struct_type) = &schema.ty else {
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
