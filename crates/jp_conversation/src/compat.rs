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

use jp_config::{
    AppConfig, PartialAppConfig, Schema, SchemaType,
    schema::{SchemaField, StructType, UnionType},
};
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
/// Walks structs, arrays, and maps, removing object keys that the matching
/// [`SchemaType::Struct`] has no field for.
/// Values the schema does not describe as one of those three shapes (leaves,
/// enums, and anything typed [`SchemaType::Unknown`], such as a tool's
/// free-form `options`) are left untouched.
///
/// A union of one type and null is an `Option`, and is walked as that type.
/// A union with several real variants is not walked at all: an object could
/// belong to any of them, and stripping it against the wrong one deletes valid
/// data.
/// `conversation.tools.<name>.enable` and `.command` are the two that reach
/// disk in that shape.
///
/// Returns the number of keys removed.
fn strip_unknown_fields(value: &mut Value, schema: &Schema) -> usize {
    match &schema.ty {
        SchemaType::Struct(struct_type) => strip_struct(value, struct_type),
        SchemaType::Array(array_type) => strip_items(value, &array_type.items_type),
        SchemaType::Object(object_type) => strip_map_values(value, &object_type.value_type),
        SchemaType::Union(union_type) => {
            sole_non_null_variant(union_type).map_or(0, |inner| strip_unknown_fields(value, inner))
        }
        _ => 0,
    }
}

/// The one variant of a union that isn't null, if there is exactly one.
///
/// Every `Option<T>` field arrives here as a two-variant union of `T` and null,
/// which is the common case by a wide margin.
fn sole_non_null_variant(union_type: &UnionType) -> Option<&Schema> {
    let mut variants = union_type
        .variants_types
        .iter()
        .map(Box::as_ref)
        .filter(|variant| !variant.is_null());

    match (variants.next(), variants.next()) {
        (Some(variant), None) => Some(variant),
        _ => None,
    }
}

/// Strip an object against a struct schema, then recurse into what remains.
///
/// A struct with a [`flatten`]ed map field absorbs every key its explicit field
/// map doesn't claim (per-tool overrides in `ToolsConfig` are the case in
/// point), so at that level nothing is unknown and the leftover keys are walked
/// against the map's value schema instead.
///
/// [`flatten`]: jp_config::schema::SchemaField::flatten
fn strip_struct(value: &mut Value, struct_type: &StructType) -> usize {
    let Some(obj) = value.as_object_mut() else {
        return 0;
    };

    let entry_schema = flattened_entry_schema(struct_type);
    let has_flatten = struct_type.fields.values().any(|f| f.flatten);

    let mut stripped = if has_flatten {
        0
    } else {
        let before = obj.len();
        obj.retain(|key, _| struct_type.fields.contains_key(key));
        before - obj.len()
    };

    for (key, child) in obj.iter_mut() {
        match struct_type.fields.get(key) {
            // The flattened field's own name is not a key in the serialized
            // form, so a key matching it is a coincidence, not that field.
            Some(field) if !field.flatten => stripped += strip_unknown_fields(child, &field.schema),
            Some(_) => {}
            None => {
                if let Some(entry_schema) = entry_schema {
                    stripped += strip_unknown_fields(child, entry_schema);
                }
            }
        }
    }

    stripped
}

/// The value schema of a struct's single flattened map field, if it has one.
///
/// `None` for a struct that flattens nothing, flattens more than one field, or
/// flattens something other than a map — in each of those cases the shape of a
/// leftover key is not knowable, and walking it against the wrong schema would
/// delete valid data.
fn flattened_entry_schema(struct_type: &StructType) -> Option<&Schema> {
    let mut flattened = struct_type
        .fields
        .values()
        .filter(|field| field.flatten)
        .map(Box::as_ref);

    match (flattened.next(), flattened.next()) {
        (Some(SchemaField { schema, .. }), None) => match &schema.ty {
            SchemaType::Object(object_type) => Some(&object_type.value_type),
            _ => None,
        },
        _ => None,
    }
}

/// Walk each element of an array against the item schema.
///
/// A vector field declared with `partial_via = MergeableVec` reaches disk
/// either as a bare array or as `{ "value": [...], "strategy": ... }`; both
/// carry the same items.
/// The wrapper's own keys are not part of the field's schema and are left
/// alone.
fn strip_items(value: &mut Value, items_schema: &Schema) -> usize {
    let items = match value {
        Value::Array(items) => items,
        Value::Object(obj) => match obj.get_mut("value") {
            Some(Value::Array(items)) => items,
            _ => return 0,
        },
        _ => return 0,
    };

    items
        .iter_mut()
        .map(|item| strip_unknown_fields(item, items_schema))
        .sum()
}

/// Walk each value of a map against the map's value schema.
///
/// Keys are entries, not fields, so none of them are stripped.
fn strip_map_values(value: &mut Value, value_schema: &Schema) -> usize {
    let Some(obj) = value.as_object_mut() else {
        return 0;
    };

    obj.values_mut()
        .map(|child| strip_unknown_fields(child, value_schema))
        .sum()
}

#[cfg(test)]
#[path = "compat_tests.rs"]
mod tests;
