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
//!
//! [`number_probes`] does the same for numbers, for the other way a value
//! reaches a field: a key-value assignment, which each field parses in its own
//! `assign` arm rather than through `Deserialize`.

use std::collections::BTreeSet;

use schematic::{
    Schema, SchemaType,
    schema::{
        ArrayType, BooleanType, EnumType, FloatType, IntegerType, LiteralValue, ObjectType,
        SchemaField, StringType, StructType, UnionType,
    },
};
use serde_json::{Value, json};

use crate::{
    AppConfig, PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
};

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
        // A collection's contents sit below the keys that identify its variant,
        // so the document is unambiguous there and the conservative fill
        // applies, exactly as it does below a struct's own keys.
        SchemaType::Object(object) => {
            path.push(Step::MapEntry);
            collect(&object.value_type, path, out, enclosing, StringFill::Skip);
            path.pop();
        }
        SchemaType::Array(array) => {
            path.push(Step::Item);
            collect(&array.items_type, path, out, enclosing, StringFill::Skip);
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

/// Every number the schema says a field accepts, as a document that writes it.
///
/// A field accepts a number when its type is an integer or a float, or a union
/// with one of those among its variants.
/// A float field is probed with an integer too, since a config file may write
/// `temperature = 1` and the deserializer reads it.
///
/// The walk follows structs, maps, array items, the value an optional wraps,
/// and the expanded form a shorthand abbreviates: the shapes a key or an object
/// reaches a field through.
/// It does not enter any other struct variant of a union, which is a table form
/// written in place of a scalar rather than a field of its own.
///
/// Documents carry no sibling keys, so each one sets its own field and nothing
/// else.
#[must_use]
pub fn number_probes(schema: &Schema) -> Vec<Probe> {
    let mut out = Vec::new();
    collect_numbers(schema, &mut Vec::new(), &mut out, &mut Vec::new());
    out
}

/// Walk a schema, recording a probe for every number a field accepts.
fn collect_numbers(
    schema: &Schema,
    path: &mut Vec<Step>,
    out: &mut Vec<Probe>,
    enclosing: &mut Vec<String>,
) {
    if let Some(name) = &schema.name {
        if enclosing.contains(name) {
            return;
        }
        enclosing.push(name.clone());
    }

    let mut record = |value: Value| {
        let probe = place(path, value.to_string(), value);
        if !out.contains(&probe) {
            out.push(probe);
        }
    };

    match &schema.ty {
        SchemaType::Integer(_) => record(json!(1)),
        SchemaType::Float(_) => {
            record(json!(1));
            record(json!(0.5));
        }
        SchemaType::Struct(struct_type) => {
            for (name, field) in &struct_type.fields {
                if field.hidden {
                    continue;
                }

                if !field.flatten {
                    path.push(Step::Field {
                        name: name.clone(),
                        siblings: serde_json::Map::new(),
                    });
                }

                collect_numbers(&field.schema, path, out, enclosing);

                if !field.flatten {
                    path.pop();
                }
            }
        }
        SchemaType::Object(object) => {
            path.push(Step::MapEntry);
            collect_numbers(&object.value_type, path, out, enclosing);
            path.pop();
        }
        SchemaType::Array(array) => {
            path.push(Step::Item);
            collect_numbers(&array.items_type, path, out, enclosing);
            path.pop();
        }
        SchemaType::Union(union) => {
            let alternatives = union
                .variants_types
                .iter()
                .filter(|variant| !variant.ty.is_null())
                .count();

            for (index, variant) in union.variants_types.iter().enumerate() {
                // A list or map variant is the plain form of a collection that
                // can also be written with its merge strategy, so its items are
                // reached either way.
                let table = matches!(variant.ty, SchemaType::Struct(_));

                if table && alternatives > 1 && union.expanded_index != Some(index) {
                    continue;
                }

                collect_numbers(variant, path, out, enclosing);
            }
        }
        _ => {}
    }

    if schema.name.is_some() {
        enclosing.pop();
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
            collect_rejections(&object.value_type, path, out, enclosing, StringFill::Skip);
            path.pop();
        }
        SchemaType::Array(array) => {
            path.push(Step::Item);
            collect_rejections(&array.items_type, path, out, enclosing, StringFill::Skip);
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

/// Build the document that writes the string `value` at `path`.
fn probe(path: &[Step], value: &str) -> Probe {
    place(path, value.to_owned(), json!(value))
}

/// Build the document that writes `value` at `path`, labelled `label` in a
/// failure message.
fn place(path: &[Step], label: String, value: Value) -> Probe {
    let mut document = value;

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
        value: label,
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

#[test]
fn writes_an_enum_value_at_its_field() {
    let schema = Schema::structure(StructType::new([(
        "mode".to_owned(),
        Schema::enumerable(EnumType::new([LiteralValue::String("fast".to_owned())])),
    )]));

    assert_eq!(probes(&schema), vec![Probe {
        path: "mode".to_owned(),
        value: "fast".to_owned(),
        document: json!({ "mode": "fast" }),
    }]);
}

#[test]
fn writes_a_probe_for_every_alias() {
    let mut variant = SchemaField::new(Schema::literal_value(LiteralValue::String(
        "strip-responses".to_owned(),
    )));
    variant.aliases = vec!["sres".to_owned()];

    let schema = Schema::enumerable(EnumType::from_fields(
        [("StripResponses".to_owned(), variant)],
        None,
    ));

    let values: Vec<_> = probes(&schema).into_iter().map(|p| p.value).collect();

    assert_eq!(values, ["strip-responses", "sres"]);
}

#[test]
fn wraps_a_value_in_the_collections_above_it() {
    let item = Schema::structure(StructType::new([(
        "kind".to_owned(),
        Schema::literal_value(LiteralValue::String("file".to_owned())),
    )]));

    let schema = Schema::structure(StructType::new([(
        "tools".to_owned(),
        Schema::object(ObjectType::new(
            Schema::string(StringType::default()),
            Schema::array(ArrayType::new(item)),
        )),
    )]));

    assert_eq!(probes(&schema), vec![Probe {
        path: "tools.probe[0].kind".to_owned(),
        value: "file".to_owned(),
        document: json!({ "tools": { "probe": [{ "kind": "file" }] } }),
    }]);
}

/// A flattened field's keys live at the parent's level on the wire, so a
/// document that nested them under the field's own name would not parse.
#[test]
fn writes_a_flattened_fields_keys_at_the_parent_level() {
    let mut flattened = SchemaField::new(Schema::object(ObjectType::new(
        Schema::string(StringType::default()),
        Schema::enumerable(EnumType::new([LiteralValue::String("on".to_owned())])),
    )));
    flattened.flatten = true;

    let schema = Schema::structure(StructType::new([("overrides".to_owned(), flattened)]));

    assert_eq!(probes(&schema)[0].document, json!({ "probe": "on" }));
}

#[test]
fn a_closed_enum_reports_its_values_and_aliases() {
    let mut variant = SchemaField::new(Schema::literal_value(LiteralValue::String(
        "always".to_owned(),
    )));
    variant.aliases = vec!["yes".to_owned()];

    let schema = Schema::enumerable(EnumType::from_fields(
        [("Always".to_owned(), variant)],
        None,
    ));

    assert_eq!(
        closed_vocabulary(&schema),
        Some(vec!["always".to_owned(), "yes".to_owned()])
    );
}

/// A catch-all variant accepts values nobody listed, so the enum as a whole
/// cannot be checked for rejection.
#[test]
fn an_enum_with_a_catch_all_is_not_closed() {
    let schema = Schema::enumerable(EnumType::from_fields(
        [
            (
                "Off".to_owned(),
                SchemaField::new(Schema::literal_value(LiteralValue::String(
                    "off".to_owned(),
                ))),
            ),
            (
                "Lines".to_owned(),
                SchemaField::new(Schema::string(StringType::default())),
            ),
        ],
        None,
    ));

    assert_eq!(closed_vocabulary(&schema), None);
}

#[test]
fn a_nullable_closed_enum_stays_closed() {
    let schema = Schema::union(UnionType::new_any([
        Schema::enumerable(EnumType::new([LiteralValue::String("on".to_owned())])),
        Schema::null(),
    ]));

    assert_eq!(closed_vocabulary(&schema), Some(vec!["on".to_owned()]));
}

#[test]
fn a_union_with_a_free_form_variant_is_not_closed() {
    let schema = Schema::union(UnionType::new_any([
        Schema::enumerable(EnumType::new([LiteralValue::String("on".to_owned())])),
        Schema::boolean(BooleanType::default()),
    ]));

    assert_eq!(closed_vocabulary(&schema), None);
}

#[test]
fn a_rejection_probe_stops_at_the_closed_node() {
    let schema = Schema::structure(StructType::new([(
        "mode".to_owned(),
        Schema::enumerable(EnumType::new([LiteralValue::String("fast".to_owned())])),
    )]));

    assert_eq!(rejections(&schema), vec![Probe {
        path: "mode".to_owned(),
        value: SENTINEL.to_owned(),
        document: json!({ "mode": SENTINEL }),
    }]);
}

/// Every string value the schema advertises has to deserialize.
///
/// The schema comes from the Rust types, the parser from `Deserialize`, and
/// only this test connects them.
/// A failure means one of the two describes a vocabulary the other does not
/// have — fix whichever is wrong rather than dropping the path from the walk.
#[test]
fn the_parser_accepts_every_value_the_schema_advertises() {
    let schema = AppConfig::schema();
    let mut failures = Vec::new();

    for probe in probes(&schema) {
        if let Err(error) = serde_json::from_value::<PartialAppConfig>(probe.document.clone()) {
            failures.push(format!(
                "{} = {:?}\n    document: {}\n    error: {error}",
                probe.path, probe.value, probe.document
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "the schema advertises {} value(s) the parser rejects:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Every value the schema says is the whole vocabulary has to be the whole
/// vocabulary.
///
/// Without this, a schema could list four spellings while the parser took
/// anything at all, and every positive probe would still pass.
#[test]
fn the_parser_rejects_values_outside_a_closed_vocabulary() {
    let schema = AppConfig::schema();
    let mut failures = Vec::new();

    for probe in rejections(&schema) {
        if serde_json::from_value::<PartialAppConfig>(probe.document.clone()).is_ok() {
            failures.push(format!("{}\n    document: {}", probe.path, probe.document));
        }
    }

    assert!(
        failures.is_empty(),
        "the schema claims a fixed set of values at {} path(s) where the parser takes \
         anything:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// The walk has to actually reach the config, not quietly cover nothing.
///
/// A cycle guard or an early return that stops too soon would leave both
/// conformance tests passing over an empty list.
#[test]
fn the_walk_reaches_the_config_it_describes() {
    let schema = AppConfig::schema();
    let paths: BTreeSet<_> = probes(&schema).into_iter().map(|p| p.path).collect();

    for expected in [
        "assistant.model.id.provider",
        "conversation.default_id",
        "conversation.tools.probe.run",
        "style.reasoning.display",
    ] {
        assert!(
            paths.contains(expected),
            "{expected} is missing from the probe walk"
        );
    }
}

#[test]
fn writes_a_number_at_an_integer_field() {
    let schema = Schema::structure(StructType::new([(
        "count".to_owned(),
        Schema::integer(IntegerType::default()),
    )]));

    assert_eq!(number_probes(&schema), vec![Probe {
        path: "count".to_owned(),
        value: "1".to_owned(),
        document: json!({ "count": 1 }),
    }]);
}

/// A file may write `temperature = 1`, so an integer is a float field's value
/// as much as a fraction is.
#[test]
fn writes_an_integer_and_a_fraction_at_a_float_field() {
    let schema = Schema::structure(StructType::new([(
        "temperature".to_owned(),
        Schema::float(FloatType::default()),
    )]));

    let documents: Vec<_> = number_probes(&schema)
        .into_iter()
        .map(|p| p.document)
        .collect();

    assert_eq!(documents, [
        json!({ "temperature": 1 }),
        json!({ "temperature": 0.5 })
    ]);
}

#[test]
fn writes_a_number_through_an_optional_struct() {
    let limits = Schema::structure(StructType::new([(
        "lines".to_owned(),
        Schema::integer(IntegerType::default()),
    )]));

    let schema = Schema::structure(StructType::new([(
        "limits".to_owned(),
        Schema::union(UnionType::new_any([limits, Schema::null()])),
    )]));

    let paths: Vec<_> = number_probes(&schema).into_iter().map(|p| p.path).collect();

    assert_eq!(paths, ["limits.lines"]);
}

/// A struct written in place of a scalar is a table form of that value, not a
/// block of fields a key can name.
#[test]
fn does_not_enter_a_table_form_of_a_scalar() {
    let table = Schema::structure(StructType::new([(
        "lines".to_owned(),
        Schema::integer(IntegerType::default()),
    )]));

    let schema = Schema::structure(StructType::new([(
        "results".to_owned(),
        Schema::union(UnionType::new_any([
            Schema::string(StringType::default()),
            table,
        ])),
    )]));

    assert_eq!(number_probes(&schema), vec![]);
}

/// Every number the schema says a field accepts has to be assignable in JSON
/// form.
///
/// A config file reaches a number field through `Deserialize`.
/// `--cfg key:=1`, a `--cfg '{...}'` object and `JP_CFG_KEY=:1` reach it
/// through the field's `assign` arm instead, and only this test holds that arm
/// to the schema.
/// A failure means the arm rejects a number the field accepts everywhere else.
#[test]
fn every_number_the_schema_advertises_is_assignable() {
    let schema = AppConfig::schema();
    let mut failures = Vec::new();

    for probe in number_probes(&schema) {
        let mut forms = vec![("object", KvAssignment::root_json(probe.document.clone()))];

        // An array item is only reachable by index once the array holds one, so
        // a path through one is assigned in the object form alone.
        if !probe.path.contains('[') {
            let kv = KvAssignment::try_from_cli(format!("{}:", probe.path), &probe.value)
                .expect("a probe value is valid JSON");
            forms.push(("key", kv));
        }

        for (form, kv) in forms {
            if let Err(error) = PartialAppConfig::empty().assign(kv) {
                failures.push(format!(
                    "{} = {} ({form} form)\n    error: {error:?}",
                    probe.path, probe.value
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} number assignment(s) the schema advertises are rejected:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// The number walk has to reach the config too, including the fields only an
/// array item or a map entry holds.
#[test]
fn the_number_walk_reaches_the_config_it_describes() {
    let schema = AppConfig::schema();
    let paths: BTreeSet<_> = number_probes(&schema).into_iter().map(|p| p.path).collect();

    for expected in [
        "assistant.model.parameters.max_tokens",
        "assistant.model.parameters.temperature",
        "conversation.compaction.rules[0].keep_first",
        "conversation.tools.probe.fan_out.concurrency",
        "style.markdown.wrap_width",
    ] {
        assert!(
            paths.contains(expected),
            "{expected} is missing from the number walk"
        );
    }
}
