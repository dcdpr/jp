//! Turn a [`Schema`] into concrete documents that exercise what it advertises.
//!
//! [`probes`] walks the schema and, for every string value it says is writable,
//! produces the smallest JSON document placing that value where the schema puts
//! it.
//! Feeding those documents to the real deserializer is what keeps the two
//! descriptions of the config's vocabulary from drifting apart: the schema is
//! derived from Rust types, the parser from `Deserialize`, and nothing else
//! forces them to agree.
//!
//! [`closed_vocabulary`] answers the other half.
//! An enum with no catch-all variant accepts exactly the values it lists, so a
//! value outside that list has to be rejected.
//! Without that check a schema could advertise four spellings while the parser
//! quietly took anything, and every positive probe would still pass.

use schematic::{
    Schema, SchemaType,
    schema::{EnumType, LiteralValue, StructType},
};
use serde_json::{Value, json};

/// One value the schema says is writable, and the document that writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    /// Where the value sits, for a failure message to point at.
    pub path: String,

    /// The value being written.
    pub value: String,

    /// The smallest document placing `value` at `path`.
    pub document: Value,
}

/// A step from a parent schema into one of its children.
#[derive(Debug, Clone)]
enum Step {
    /// A named field of a struct, alongside the sibling keys its type needs.
    ///
    /// A struct reached through an internal tag or an untagged union is only
    /// recognised once enough of it is present: the tag that names the variant,
    /// the path a strategy qualifies.
    /// Carrying those siblings keeps a probe a test of the value at the end of
    /// the path rather than of whether the document around it happened to be
    /// well-formed.
    Field {
        name: String,
        siblings: serde_json::Map<String, Value>,
    },

    /// The value side of a map, reached under an arbitrary key.
    MapEntry,

    /// An element of an array.
    Item,
}

/// The map key every probe uses, so a failure names something searchable.
const PROBE_KEY: &str = "probe";

/// Every string value the schema advertises, as a document that writes it.
///
/// Aliases count: a spelling listed beside a variant is a spelling the schema
/// promises the parser takes.
#[must_use]
pub fn probes(schema: &Schema) -> Vec<Probe> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    let mut enclosing = Vec::new();

    collect(
        schema,
        &mut path,
        &mut out,
        &mut enclosing,
        StringFill::Skip,
    );
    out
}

/// The exact set of values a schema accepts, when that set is finite.
///
/// `None` when any variant admits values beyond a fixed list — a string, a
/// number, a struct, an unknown — because then no value can be assumed
/// invalid.
/// A `null` variant is ignored, since it makes the field optional rather than
/// open-ended.
#[must_use]
pub fn closed_vocabulary(schema: &Schema) -> Option<Vec<String>> {
    match &schema.ty {
        SchemaType::Literal(literal) => match &literal.value {
            LiteralValue::String(value) => Some(vec![value.clone()]),
            _ => None,
        },
        SchemaType::Enum(enum_type) => closed_enum_vocabulary(enum_type),
        SchemaType::Union(union) => {
            let mut values = Vec::new();

            for variant in &union.variants_types {
                if variant.ty.is_null() {
                    continue;
                }

                values.extend(closed_vocabulary(variant)?);
            }

            (!values.is_empty()).then_some(values)
        }
        _ => None,
    }
}

/// The values a closed enum accepts, including every alias.
fn closed_enum_vocabulary(enum_type: &EnumType) -> Option<Vec<String>> {
    // A hand-written schema built from `EnumType::new` carries values without
    // per-variant detail. Those values are the whole vocabulary, since there is
    // no variant there that could widen it.
    let Some(variants) = enum_type.variants.as_ref() else {
        return enum_type
            .values
            .iter()
            .map(|value| match value {
                LiteralValue::String(value) => Some(value.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .filter(|values| !values.is_empty());
    };

    let mut values = Vec::new();

    for field in variants.values() {
        let SchemaType::Literal(literal) = &field.schema.ty else {
            // A variant that is not a fixed value (a catch-all accepting any
            // string) opens the whole enum up.
            return None;
        };

        let LiteralValue::String(value) = &literal.value else {
            return None;
        };

        values.push(value.clone());
        values.extend(field.aliases.iter().cloned());
    }

    (!values.is_empty()).then_some(values)
}

/// Walk a schema, recording a probe for every string value it advertises.
fn collect(
    schema: &Schema,
    path: &mut Vec<Step>,
    out: &mut Vec<Probe>,
    enclosing: &mut Vec<String>,
    strings: StringFill,
) {
    if let Some(name) = &schema.name {
        if enclosing.contains(name) {
            return;
        }
        enclosing.push(name.clone());
    }

    collect_type(schema, path, out, enclosing, strings);

    if schema.name.is_some() {
        enclosing.pop();
    }
}

/// Walk a schema's shape, recording probes and descending into its children.
fn collect_type(
    schema: &Schema,
    path: &mut Vec<Step>,
    out: &mut Vec<Probe>,
    enclosing: &mut Vec<String>,
    strings: StringFill,
) {
    match &schema.ty {
        SchemaType::Literal(literal) => {
            if let LiteralValue::String(value) = &literal.value {
                out.push(probe(path, value));
            }
        }
        SchemaType::Enum(enum_type) => collect_enum(enum_type, path, out),
        SchemaType::Struct(struct_type) => {
            for (name, field) in &struct_type.fields {
                if field.hidden {
                    continue;
                }

                // A flattened field's keys sit at the parent's level, which is
                // where a document has to put them for the parser to find them.
                if !field.flatten {
                    path.push(Step::Field {
                        name: name.clone(),
                        siblings: siblings_of(struct_type, name, strings),
                    });
                }

                // Below this struct's own keys the document is unambiguous
                // again, so the conservative fill applies from here down.
                collect(&field.schema, path, out, enclosing, StringFill::Skip);

                if !field.flatten {
                    path.pop();
                }
            }
        }
        SchemaType::Object(object) => {
            path.push(Step::MapEntry);
            collect(&object.value_type, path, out, enclosing, strings);
            path.pop();
        }
        SchemaType::Array(array) => {
            path.push(Step::Item);
            collect(&array.items_type, path, out, enclosing, strings);
            path.pop();
        }
        SchemaType::Union(union) => {
            for variant in &union.variants_types {
                collect(variant, path, out, enclosing, StringFill::Invent);
            }
        }
        // A reference names a type already being walked further up, so its
        // values are recorded there.
        _ => {}
    }
}

/// The smallest values for every field of `struct_type` other than `skip`.
///
/// Fields whose shape has no obvious smallest value are left out; a partial
/// config treats an absent key as unset, so omitting one is safe where
/// inventing a value would not be.
fn siblings_of(
    struct_type: &StructType,
    skip: &str,
    strings: StringFill,
) -> serde_json::Map<String, Value> {
    let mut siblings = serde_json::Map::new();

    for (name, field) in &struct_type.fields {
        if name == skip || field.hidden || field.flatten {
            continue;
        }

        if let Some(value) = smallest_value(&field.schema, strings) {
            siblings.insert(name.clone(), value);
        }
    }

    siblings
}

/// Whether a field typed as a plain string may be filled with a made-up value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringFill {
    /// Leave it out.
    /// A schema saying `string` may sit in front of a parser that accepts a
    /// fixed set of words, and inventing one would fail for a reason that has
    /// nothing to do with the value under test.
    Skip,

    /// Fill it.
    /// Inside an untagged union no field stands alone: the document has to
    /// carry enough of the variant to be recognised as that variant at all, so
    /// leaving a string out fails just as surely.
    Invent,
}

/// The least elaborate value a schema accepts, when one is obvious.
///
/// Structs collapse to `{}` rather than being filled out: every field of a
/// partial is optional, so an empty table satisfies one without this having to
/// know anything about what it contains.
fn smallest_value(schema: &Schema, strings: StringFill) -> Option<Value> {
    match &schema.ty {
        SchemaType::Boolean(_) => Some(json!(true)),
        SchemaType::Integer(_) => Some(json!(1)),
        SchemaType::Float(_) => Some(json!(1.0)),
        SchemaType::String(_) => (strings == StringFill::Invent).then(|| json!("x")),
        SchemaType::Array(_) => Some(json!([])),
        SchemaType::Object(_) => Some(json!({})),
        SchemaType::Struct(struct_type) => {
            // Every field of a partial is optional unless the schema says
            // otherwise, so an empty table usually does. A struct that names
            // required fields has to carry them.
            let mut object = serde_json::Map::new();

            for name in struct_type.required.iter().flatten() {
                let field = struct_type.fields.get(name)?;
                object.insert(
                    name.clone(),
                    smallest_value(&field.schema, StringFill::Invent)?,
                );
            }

            Some(Value::Object(object))
        }
        SchemaType::Literal(literal) => match &literal.value {
            LiteralValue::String(value) => Some(json!(value)),
            LiteralValue::Bool(value) => Some(json!(value)),
            LiteralValue::Int(value) => Some(json!(value)),
            LiteralValue::UInt(value) => Some(json!(value)),
            LiteralValue::F32(value) => Some(json!(value)),
            LiteralValue::F64(value) => Some(json!(value)),
        },
        SchemaType::Enum(enum_type) => enum_type.values.first().and_then(|value| match value {
            LiteralValue::String(value) => Some(json!(value)),
            _ => None,
        }),
        SchemaType::Union(union) => union
            .variants_types
            .iter()
            .filter(|variant| !variant.ty.is_null())
            .find_map(|variant| smallest_value(variant, strings)),
        SchemaType::Null
        | SchemaType::Unknown
        | SchemaType::Reference(_)
        | SchemaType::Tuple(_) => None,
    }
}

/// Record one probe per value an enum advertises, aliases included.
fn collect_enum(enum_type: &EnumType, path: &[Step], out: &mut Vec<Probe>) {
    let Some(variants) = &enum_type.variants else {
        for value in &enum_type.values {
            if let LiteralValue::String(value) = value {
                out.push(probe(path, value));
            }
        }
        return;
    };

    for field in variants.values() {
        let SchemaType::Literal(literal) = &field.schema.ty else {
            continue;
        };

        let LiteralValue::String(value) = &literal.value else {
            continue;
        };

        out.push(probe(path, value));

        for alias in &field.aliases {
            out.push(probe(path, alias));
        }
    }
}

/// The value used to check that a closed vocabulary really is closed.
pub const SENTINEL: &str = "__jp_schema_probe_invalid__";

/// A document per closed vocabulary in the schema, writing a value outside it.
///
/// Each of these must be *rejected*.
/// A path whose schema admits any string is left out, since nothing there can
/// be assumed invalid.
#[must_use]
pub fn rejections(schema: &Schema) -> Vec<Probe> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    let mut enclosing = Vec::new();

    collect_rejections(
        schema,
        &mut path,
        &mut out,
        &mut enclosing,
        StringFill::Skip,
    );
    out
}

/// Walk a schema, stopping at each node whose accepted values are finite.
fn collect_rejections(
    schema: &Schema,
    path: &mut Vec<Step>,
    out: &mut Vec<Probe>,
    enclosing: &mut Vec<String>,
    strings: StringFill,
) {
    if closed_vocabulary(schema).is_some() {
        out.push(probe(path, SENTINEL));
        return;
    }

    if let Some(name) = &schema.name {
        if enclosing.contains(name) {
            return;
        }
        enclosing.push(name.clone());
    }

    match &schema.ty {
        SchemaType::Struct(struct_type) => {
            for (name, field) in &struct_type.fields {
                if field.hidden {
                    continue;
                }

                if !field.flatten {
                    path.push(Step::Field {
                        name: name.clone(),
                        siblings: siblings_of(struct_type, name, strings),
                    });
                }

                collect_rejections(&field.schema, path, out, enclosing, StringFill::Skip);

                if !field.flatten {
                    path.pop();
                }
            }
        }
        SchemaType::Object(object) => {
            path.push(Step::MapEntry);
            collect_rejections(&object.value_type, path, out, enclosing, strings);
            path.pop();
        }
        SchemaType::Array(array) => {
            path.push(Step::Item);
            collect_rejections(&array.items_type, path, out, enclosing, strings);
            path.pop();
        }
        SchemaType::Union(union) => {
            // The union as a whole is open, or the check above would have
            // stopped here. Descending into a scalar variant would claim a
            // fixed vocabulary at a path where a sibling variant takes anything
            // — a container variant has its own keys, which stay checkable.
            for variant in &union.variants_types {
                if matches!(variant.ty, SchemaType::Struct(_) | SchemaType::Object(_)) {
                    collect_rejections(variant, path, out, enclosing, StringFill::Invent);
                }
            }
        }
        _ => {}
    }

    if schema.name.is_some() {
        enclosing.pop();
    }
}

/// Build the document that writes `value` at `path`.
fn probe(path: &[Step], value: &str) -> Probe {
    let mut document = json!(value);

    for step in path.iter().rev() {
        document = match step {
            Step::Field { name, siblings } => {
                let mut object = siblings.clone();
                object.insert(name.clone(), document);
                Value::Object(object)
            }
            Step::MapEntry => json!({ PROBE_KEY: document }),
            Step::Item => json!([document]),
        };
    }

    Probe {
        path: render_path(path),
        value: value.to_owned(),
        document,
    }
}

/// A readable location for a failure message.
fn render_path(path: &[Step]) -> String {
    let mut rendered = String::new();

    for step in path {
        match step {
            Step::Field { name, .. } => {
                if !rendered.is_empty() {
                    rendered.push('.');
                }
                rendered.push_str(name);
            }
            Step::MapEntry => {
                rendered.push('.');
                rendered.push_str(PROBE_KEY);
            }
            Step::Item => rendered.push_str("[0]"),
        }
    }

    rendered
}

#[cfg(test)]
#[path = "schema_probe_tests.rs"]
mod tests;
