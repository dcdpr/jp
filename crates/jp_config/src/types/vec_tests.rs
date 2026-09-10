use schematic::{Schema, SchemaBuilder, SchemaType};

use super::*;

/// Flatten a schema into the shapes it accepts, one name each.
fn describe(schema: &Schema, out: &mut Vec<String>) {
    match &schema.ty {
        SchemaType::Union(union) => {
            for variant in &union.variants_types {
                describe(variant, out);
            }
        }
        SchemaType::Enum(enum_type) => {
            for value in &enum_type.values {
                out.push(value.to_string());
            }
        }
        SchemaType::Boolean(_) => out.push("bool".to_owned()),
        SchemaType::Null => out.push("null".to_owned()),
        other => panic!("unexpected shape: {other:?}"),
    }
}

/// The shapes a named field of a struct schema accepts, flattened.
fn field_shapes(schema: &Schema, field: &str) -> Vec<String> {
    let SchemaType::Struct(struct_type) = &schema.ty else {
        panic!("expected a struct");
    };

    let field = struct_type.fields.get(field).expect("the field exists");

    let mut out = Vec::new();
    describe(&field.schema, &mut out);
    out
}

/// `dedup` takes more than its `Option<bool>` type says, and the schema has to
/// say so too.
///
/// `deserialize_dedup` also accepts `"inherit"`, `"true"` and `"false"`.
/// The field type cannot express those, so they are declared alongside it.
#[test]
fn the_dedup_schema_describes_every_shape_the_parser_takes() {
    let schema = SchemaBuilder::build_root::<MergedVec<String>>();

    assert_eq!(field_shapes(&schema, "dedup"), [
        "\"inherit\"",
        "\"true\"",
        "\"false\"",
        "bool",
        "null"
    ]);
}

/// A layer reads `dedup` the same way a resolved config does.
///
/// The custom deserializer is declared in serde's namespace rather than
/// schematic's, and the generated partial has to pick it up from there too.
/// Without that, `PartialMergedVec` falls back to a plain `Option<bool>` and
/// rejects the string forms the resolved type accepts.
#[test]
fn the_partial_reads_dedup_the_same_way() {
    for (input, expected) in [
        (r#"{"dedup":true}"#, Some(true)),
        (r#"{"dedup":"true"}"#, Some(true)),
        (r#"{"dedup":"false"}"#, Some(false)),
        (r#"{"dedup":"inherit"}"#, None),
        (r"{}", None),
    ] {
        let parsed: PartialMergedVec<String> = serde_json::from_str(input).unwrap();

        assert_eq!(parsed.dedup, expected, "parsing {input}");
    }
}

/// Each shape the schema lists has to survive a round trip through the parser.
///
/// Deserialization goes through `MergeableVec`, which is the type a config
/// field holds; it recognises the wrapper by its keys and hands the rest to
/// `MergedVec`.
#[test]
fn the_parser_takes_every_dedup_shape_the_schema_lists() {
    for (input, expected) in [
        (r#"{"value":[],"dedup":true}"#, Some(true)),
        (r#"{"value":[],"dedup":false}"#, Some(false)),
        (r#"{"value":[],"dedup":"true"}"#, Some(true)),
        (r#"{"value":[],"dedup":"false"}"#, Some(false)),
        (r#"{"value":[],"dedup":"inherit"}"#, None),
        (r#"{"value":[]}"#, None),
    ] {
        let parsed: MergeableVec<String> = serde_json::from_str(input).unwrap();

        let MergeableVec::Merged(merged) = parsed else {
            panic!("the wrapper form parses as `Merged`: {input}");
        };

        assert_eq!(merged.dedup, expected, "parsing {input}");
    }
}
