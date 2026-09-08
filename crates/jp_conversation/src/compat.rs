//! Backward-compatible deserialization for [`PartialAppConfig`].
//!
//! When the [`AppConfig`] schema evolves (fields added, removed, or renamed),
//! old conversation data may reference fields that no longer exist.
//! The standard serde `deny_unknown_fields` on `Partial*Config` types causes
//! deserialization to fail entirely.
//!
//! Recovery happens in two passes.
//! The first is schema-aware stripping: we walk the JSON value alongside the
//! current `AppConfig` schema and remove any keys that don't exist in the
//! schema.
//! The second catches what a schema walk cannot — a field whose type changed,
//! an enum variant that was renamed, a validator that was tightened — by
//! deserializing repeatedly and dropping the field each failure names, until
//! the value parses.
//!
//! Only a value that cannot be recovered at all falls back to an empty config.
//! That is close to useless as a recovery: `assistant.model.id.provider` and
//! `conversation.tools.'*'.run` are required and have no default, so an empty
//! config cannot be finalized and the conversation still fails to load.
//! Both passes exist to keep that last resort from being reached.

use jp_config::{
    AppConfig, PartialAppConfig, Schema, SchemaType,
    schema::{ReferenceType, SchemaField, StructType, UnionType},
};
use serde_json::{Value, json};
use serde_path_to_error::{Path as ErrorPath, Segment};
use tracing::warn;

/// Deserialize a [`PartialAppConfig`] from a raw JSON value, tolerating schema
/// changes.
///
/// 1. Strips unknown fields using the current [`AppConfig`] schema.
/// 2. Repairs field values whose accepted spelling has changed.
/// 3. Deserializes, dropping each field the failure names and retrying, until
///    the value parses or nothing is left to drop.
/// 4. Falls back to [`PartialAppConfig::empty()`] only when a failure names
///    nothing droppable.
///
/// Every dropped field is reported at warn level with its path.
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

    // Each pass either succeeds or removes one entry from `value`, so the
    // value shrinks until it parses or has nothing left to give.
    loop {
        let error = match serde_path_to_error::deserialize::<_, PartialAppConfig>(&value) {
            Ok(config) => return config,
            Err(error) => error,
        };

        if !remove_at_path(&mut value, error.path()) {
            warn!(
                error = %error.inner(),
                "Stored config cannot be recovered, replacing with empty config.",
            );
            return PartialAppConfig::empty();
        }

        warn!(
            path = %error.path(),
            error = %error.inner(),
            "Dropped an incompatible field from the stored config.",
        );
    }
}

/// Remove the entry a deserialization failure points at.
///
/// Returns `false` when the path names nothing removable — an empty path (the
/// failure is the value itself), a path that no longer resolves, or one made
/// only of enum and unknown segments.
///
/// A path that *ends* in an enum or unknown segment is truncated to its last
/// map or sequence segment and that entry is removed instead.
/// Dropping the enclosing field discards more than the failure named, but it
/// keeps the rest of the config, which the alternative does not.
fn remove_at_path(value: &mut Value, path: &ErrorPath) -> bool {
    let segments: Vec<&Segment> = path.iter().collect();

    // `Chain::Some`, `NonStringKey` and the like arrive as `Unknown`, and an
    // enum variant is not an addressable entry either.
    let removable = segments
        .iter()
        .rposition(|segment| matches!(segment, Segment::Map { .. } | Segment::Seq { .. }));
    let Some(removable) = removable else {
        return false;
    };
    let (target, parents) = segments[..=removable]
        .split_last()
        .expect("rposition found an index, so the slice is non-empty");

    let mut current = value;
    for segment in parents {
        let child = match segment {
            Segment::Map { key } => current.as_object_mut().and_then(|obj| obj.get_mut(key)),
            Segment::Seq { index } => current
                .as_array_mut()
                .and_then(|items| items.get_mut(*index)),
            Segment::Enum { .. } | Segment::Unknown => None,
        };

        let Some(child) = child else {
            return false;
        };
        current = child;
    }

    match target {
        Segment::Map { key } => current
            .as_object_mut()
            .is_some_and(|obj| obj.remove(key).is_some()),
        Segment::Seq { index } => current.as_array_mut().is_some_and(|items| {
            let in_range = *index < items.len();
            if in_range {
                items.remove(*index);
            }
            in_range
        }),
        Segment::Enum { .. } | Segment::Unknown => false,
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
/// A [`SchemaType::Reference`] is followed to the type it names, so a
/// self-referential type is walked to whatever depth the value goes.
/// Values the schema describes as none of those (leaves, enums, and anything
/// typed [`SchemaType::Unknown`], such as a tool's free-form `options`) are
/// left untouched.
///
/// A union is walked as the one variant that could hold the value in hand.
/// When two variants could, the union is left alone: the value could belong to
/// either, and stripping it against the wrong one deletes valid data.
///
/// Returns the number of keys removed.
fn strip_unknown_fields(value: &mut Value, schema: &Schema) -> usize {
    strip_schema(value, schema, &mut Vec::new())
}

/// The named schemas enclosing the current position, innermost last.
///
/// A schema builder that meets a type it is already describing emits a
/// [`SchemaType::Reference`] to that type's name instead of expanding it again,
/// so a reference always names one of these.
type Enclosing<'a> = Vec<(&'a str, &'a Schema)>;

/// Walk a value against a schema, recording the schema's name for any reference
/// below it to resolve against.
fn strip_schema<'a>(value: &mut Value, schema: &'a Schema, enclosing: &mut Enclosing<'a>) -> usize {
    let name = schema.name.as_deref();
    if let Some(name) = name {
        enclosing.push((name, schema));
    }

    let stripped = strip_schema_type(value, &schema.ty, enclosing);

    if name.is_some() {
        enclosing.pop();
    }

    stripped
}

/// Walk a value against a schema's shape.
fn strip_schema_type<'a>(
    value: &mut Value,
    ty: &'a SchemaType,
    enclosing: &mut Enclosing<'a>,
) -> usize {
    match ty {
        SchemaType::Struct(struct_type) => strip_struct(value, struct_type, enclosing),
        SchemaType::Array(array_type) => strip_items(value, &array_type.items_type, enclosing),
        SchemaType::Object(object_type) => {
            strip_map_values(value, &object_type.value_type, enclosing)
        }
        SchemaType::Union(union_type) => sole_matching_variant(union_type, value)
            .map_or(0, |inner| strip_schema(value, inner, enclosing)),
        // A recursive type (`conversation.tools.<name>.parameters.<name>` is
        // the one that reaches disk, through `items` and `properties`) is
        // described once and referred to by name below that.
        //
        // Resolving to the named schema's shape rather than back through
        // [`strip_schema`] keeps a reference from resolving to another
        // reference, so this cannot cycle without descending into the value.
        SchemaType::Reference(reference) => resolve(reference, enclosing)
            .map_or(0, |target| strip_schema_type(value, target, enclosing)),
        _ => 0,
    }
}

/// The shape of the innermost enclosing schema a reference names.
///
/// `None` for a name that is not enclosing, which no schema this walks should
/// produce; the value is left untouched rather than guessed at.
fn resolve<'a>(reference: &ReferenceType, enclosing: &Enclosing<'a>) -> Option<&'a SchemaType> {
    enclosing
        .iter()
        .rev()
        .find(|(name, _)| *name == reference.name)
        .map(|&(_, schema)| &schema.ty)
}

/// The one variant of a union to walk `value` against, if there is one.
///
/// A union of one type and null is an `Option`, and the value selects that type
/// by being present at all, whatever shape it arrived in.
/// That matters for a field declared with `partial_via = MergeableVec`: its
/// schema says array while the value can be the `{ "value": [...] }` object,
/// and [`strip_items`] is what knows how to reconcile the two.
///
/// A union with several real variants is separated by shape instead.
/// A table written at `conversation.tools.<name>.enable` can only be the `{
/// state, allow_toggle }` struct, never the bool or the legacy strings beside
/// it.
/// Ambiguity there yields `None` rather than a guess, so a union of two tables
/// is left untouched.
fn sole_matching_variant<'a>(union_type: &'a UnionType, value: &Value) -> Option<&'a Schema> {
    let variants = || union_type.variants_types.iter().map(Box::as_ref);

    sole(variants().filter(|variant| !variant.is_null()))
        .or_else(|| strategy_carrying_variant(union_type, value))
        .or_else(|| sole(variants().filter(|variant| accepts(&variant.ty, value))))
}

/// The variant of a collection that can state its own merge strategy.
///
/// A `MergeableMap` is the plain map beside a wrapper struct holding it under
/// `value` next to the strategy.
/// Both are objects on the wire, so shape alone leaves the union ambiguous, and
/// every key inside a tool, server, alias or plugin would go unwalked: a stale
/// one would then survive to fail typed deserialization, which discards the
/// whole stored config rather than the key.
///
/// Told apart the way the wrapper's own deserializer does it: an object
/// carrying both `value` and `strategy` is the wrapper, anything else is the
/// map.
/// An entry named `value` needs the sibling `strategy` before it reads as the
/// wrapper, which is what keeps a tool called `value` addressable.
///
/// The pair is recognised by one side being a map rather than by the wrapper's
/// own fields: the wrapper type is described once and referred to by name
/// wherever it appears again, so most of its uses are a reference with no
/// fields to inspect.
fn strategy_carrying_variant<'a>(union_type: &'a UnionType, value: &Value) -> Option<&'a Schema> {
    let variants = || {
        union_type
            .variants_types
            .iter()
            .map(Box::as_ref)
            .filter(|variant| !variant.is_null())
    };

    let is_map = |schema: &Schema| matches!(schema.ty, SchemaType::Object(_));

    let collection = sole(variants().filter(|variant| is_map(variant)))?;
    let wrapper = sole(variants().filter(|variant| !is_map(variant)))?;

    let stated = value
        .as_object()
        .is_some_and(|obj| obj.contains_key("value") && obj.contains_key("strategy"));

    Some(if stated { wrapper } else { collection })
}

/// The only item an iterator yields, if it yields exactly one.
fn sole<'a>(mut variants: impl Iterator<Item = &'a Schema>) -> Option<&'a Schema> {
    match (variants.next(), variants.next()) {
        (Some(variant), None) => Some(variant),
        _ => None,
    }
}

/// Whether a schema could describe a JSON value of this shape.
///
/// Only consulted for a union with more than one variant that isn't null, so a
/// value whose shape matches nothing leaves that union alone rather than
/// selecting badly.
///
/// Shape only: a string schema accepts every string, whatever its constraints.
/// A union and an unknown could hold anything, so they accept everything, which
/// makes them count as candidates and pushes the enclosing union towards being
/// left alone.
///
/// A reference counts for the same reason without being resolved: the shape it
/// names matters only once a variant has been chosen, and resolving one here
/// would need the enclosing schemas that [`strip_schema_type`] carries.
const fn accepts(ty: &SchemaType, value: &Value) -> bool {
    matches!(
        (ty, value),
        (SchemaType::Null, Value::Null)
            | (SchemaType::Boolean(_), Value::Bool(_))
            | (
                SchemaType::Integer(_) | SchemaType::Float(_),
                Value::Number(_)
            )
            | (
                SchemaType::String(_) | SchemaType::Enum(_) | SchemaType::Literal(_),
                Value::String(_),
            )
            | (SchemaType::Array(_) | SchemaType::Tuple(_), Value::Array(_))
            | (
                SchemaType::Struct(_) | SchemaType::Object(_),
                Value::Object(_)
            )
            | (
                SchemaType::Union(_) | SchemaType::Reference(_) | SchemaType::Unknown,
                _
            )
    )
}

/// Strip an object against a struct schema, then recurse into what remains.
///
/// A struct with a [`flatten`]ed map field absorbs every key its explicit field
/// map doesn't claim (per-tool overrides in `ToolsConfig` are the case in
/// point), so at that level nothing is unknown and the leftover keys are walked
/// against the map's value schema instead.
///
/// [`flatten`]: jp_config::schema::SchemaField::flatten
fn strip_struct<'a>(
    value: &mut Value,
    struct_type: &'a StructType,
    enclosing: &mut Enclosing<'a>,
) -> usize {
    let Some(obj) = value.as_object_mut() else {
        return 0;
    };

    let flattened = flattened_field_schema(struct_type);
    let entry_schema = flattened.and_then(map_value_schema);
    let has_flatten = struct_type.fields.values().any(|f| f.flatten);

    let mut stripped = if has_flatten {
        0
    } else {
        let before = obj.len();
        obj.retain(|key, _| struct_type.fields.contains_key(key));
        before - obj.len()
    };

    for (key, child) in obj.iter_mut() {
        // The flattened field's own name is not a key in the serialized form,
        // so a key matching it is an entry of the map it flattens, not that
        // field.
        match struct_type.fields.get(key).filter(|field| !field.flatten) {
            Some(field) => stripped += strip_schema(child, &field.schema, enclosing),
            None => {
                if let Some(entry_schema) = entry_schema {
                    stripped += strip_schema(child, entry_schema, enclosing);
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
fn flattened_field_schema(struct_type: &StructType) -> Option<&Schema> {
    let mut flattened = struct_type
        .fields
        .values()
        .filter(|field| field.flatten)
        .map(Box::as_ref);

    match (flattened.next(), flattened.next()) {
        (Some(SchemaField { schema, .. }), None) => Some(schema),
        _ => None,
    }
}

/// The schema of a map's values, for a map written either plainly or with a
/// stated merge strategy.
///
/// A map that can state one is a union of the plain map and the wrapper holding
/// it under `value`.
/// Flattened, its entries are sibling keys of the struct around it, which is
/// the plain map's shape, so that is the variant their values are walked
/// against.
fn map_value_schema(schema: &Schema) -> Option<&Schema> {
    fn value_type(ty: &SchemaType) -> Option<&Schema> {
        match ty {
            SchemaType::Object(object_type) => Some(&object_type.value_type),
            _ => None,
        }
    }

    match &schema.ty {
        SchemaType::Union(union_type) => union_type
            .variants_types
            .iter()
            .find_map(|variant| value_type(&variant.ty)),
        ty => value_type(ty),
    }
}

/// Walk each element of an array against the item schema.
///
/// A vector field declared with `partial_via = MergeableVec` reaches disk
/// either as a bare array or as `{ "value": [...], "strategy": ... }`; both
/// carry the same items.
/// The wrapper's own keys are not part of the field's schema and are left
/// alone.
fn strip_items<'a>(
    value: &mut Value,
    items_schema: &'a Schema,
    enclosing: &mut Enclosing<'a>,
) -> usize {
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
        .map(|item| strip_schema(item, items_schema, enclosing))
        .sum()
}

/// Walk each value of a map against the map's value schema.
///
/// Keys are entries, not fields, so none of them are stripped.
fn strip_map_values<'a>(
    value: &mut Value,
    value_schema: &'a Schema,
    enclosing: &mut Enclosing<'a>,
) -> usize {
    let Some(obj) = value.as_object_mut() else {
        return 0;
    };

    obj.values_mut()
        .map(|child| strip_schema(child, value_schema, enclosing))
        .sum()
}

#[cfg(test)]
#[path = "compat_tests.rs"]
mod tests;
