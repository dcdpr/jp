//! Backward-compatible deserialization for [`PartialAppConfig`].
//!
//! When the [`AppConfig`] schema evolves (fields added, removed, or renamed),
//! old conversation data may reference fields that no longer exist.
//! The standard serde `deny_unknown_fields` on `Partial*Config` types causes
//! deserialization to fail entirely.
//!
//! This module provides schema-aware stripping: before deserializing, we walk
//! the JSON value alongside the current `AppConfig` schema and remove any keys
//! that don't exist in the schema.
//! If deserialization still fails after stripping (e.g. a field's type
//! changed), we fall back to an empty config.

use jp_config::{AppConfig, PartialAppConfig, Schema, SchemaType};
use serde_json::{Value, json};
use tracing::warn;

/// Deserialize a [`PartialAppConfig`] from a raw JSON value, tolerating schema
/// changes.
///
/// 1. Strips unknown fields using the current [`AppConfig`] schema.
/// 2. Repairs field values whose accepted spelling has changed.
/// 3. Attempts typed deserialization.
/// 4. If that fails (e.g. a field's type changed), falls back to
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

    migrate_legacy_rule_bounds(&mut value);

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

/// Repair compaction rule bounds written in a spelling the current parser no
/// longer accepts.
///
/// `keep_first`/`keep_last` once accepted `"last"` for the last-compaction
/// marker and `"@N"` for an absolute turn, and both forms reached disk.
/// Left as-is they fail typed deserialization, which discards the entire stored
/// config — model, tools, style and all — so they are rewritten here:
///
/// - `"last"` becomes `"last-compaction"`, the same bound under its current
///   name.
/// - A rule carrying an `"@N"` bound is dropped whole.
///   An absolute turn describes one conversation, so a config rule has no way
///   to express it, and removing just the bound would leave the rule running
///   over the default range instead — compacting turns the rule never named.
///   Dropping the rule compacts less than intended rather than more, and the
///   config around it still survives.
fn migrate_legacy_rule_bounds(value: &mut Value) {
    let Some(rules) = value.pointer_mut("/conversation/compaction/rules") else {
        return;
    };

    // Mirrors `MergeableVec::is_empty`, which is what decides whether an
    // item-less list reads as "unset" further down.
    let has_active_metadata = match &*rules {
        Value::Object(obj) => {
            obj.get("strategy").is_some_and(|v| !v.is_null())
                || obj.get("dedup").is_some_and(|v| !v.is_null())
                || obj.get("discard_when_merged") == Some(&Value::Bool(true))
        }
        _ => false,
    };

    // `rules` is a `MergeableVec`: a bare array, or an object whose `value` key
    // holds one (the shape `ConversationStream::to_parts` writes).
    let items = match rules {
        Value::Array(items) => items,
        Value::Object(obj) => match obj.get_mut("value") {
            Some(Value::Array(items)) => items,
            _ => return,
        },
        _ => return,
    };

    let before = items.len();

    items.retain_mut(|rule| {
        let Some(rule) = rule.as_object_mut() else {
            return true;
        };

        let mut keep = true;
        for key in ["keep_first", "keep_last"] {
            let Some(bound) = rule.get(key).and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };

            if bound.eq_ignore_ascii_case("last") {
                warn!(
                    field = key,
                    "Renaming stored `last` bound to `last-compaction`."
                );
                rule.insert(key.to_owned(), Value::String("last-compaction".to_owned()));
            } else if bound.starts_with('@') {
                warn!(
                    field = key,
                    bound = bound,
                    "Dropping stored compaction rule: config rules cannot name an absolute turn, \
                     and running the rule with a substituted bound would compact a range it never \
                     named.",
                );
                keep = false;
            }
        }

        keep
    });

    // A list with no items and no merge metadata reads as "unset" to
    // `PartialCompactionConfig::fill_from`, which then reinstates the built-in
    // default rule — compacting more than the rule just dropped. An explicit
    // strategy makes it read as "no rules" instead.
    //
    // A list that already carries metadata keeps it: its strategy decides how
    // the now-empty list merges, and forcing `replace` here would wipe rules
    // set by a lower layer.
    if before > 0 && items.is_empty() && !has_active_metadata {
        *rules = json!({ "value": [], "strategy": "replace" });
    }
}

/// Recursively strip JSON object keys that don't exist in the schema.
///
/// At each [`SchemaType::Struct`] level, retains only keys present in the
/// schema's field map and recurses into nested struct fields.
/// Non-struct values (leaves, arrays, enums) are left untouched.
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
