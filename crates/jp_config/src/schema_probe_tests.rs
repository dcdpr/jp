use std::collections::BTreeSet;

use schematic::{
    Schema,
    schema::{
        ArrayType, BooleanType, EnumType, FloatType, IntegerType, LiteralValue, ObjectType,
        SchemaField, StringType, StructType, UnionType,
    },
};
use serde_json::json;

use super::{Probe, SENTINEL, closed_vocabulary, number_probes, probes, rejections};
use crate::{
    AppConfig, PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
};

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
