use schematic::{Schema, SchemaBuilder, SchemaType};

use super::*;

/// The shapes a named field of a struct schema accepts, flattened.
///
/// A union contributes each of its variants; anything else contributes itself.
fn field_shapes(schema: &Schema, field: &str) -> Vec<String> {
    let SchemaType::Struct(struct_type) = &schema.ty else {
        panic!("expected a struct");
    };

    let field = struct_type.fields.get(field).expect("the field exists");

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

    let mut out = Vec::new();
    describe(&field.schema, &mut out);
    out
}

/// `dedup` takes more than its `Option<StringDedup>` type says, and the schema
/// has to say so too.
///
/// `deserialize_string_dedup` also accepts the boolean shorthand the field's
/// documentation advertises, and `"inherit"`.
/// The field type cannot express either, so they are declared alongside it.
#[test]
fn the_dedup_schema_describes_every_shape_the_parser_takes() {
    let schema = SchemaBuilder::build_root::<MergedString>();

    assert_eq!(field_shapes(&schema, "dedup"), [
        "bool",
        "\"inherit\"",
        "\"off\"",
        "\"exact\"",
        "\"block\"",
        "\"contains\"",
        "null"
    ]);
}

/// Each shape the schema lists has to survive a round trip through the parser.
#[test]
fn the_parser_takes_every_dedup_shape_the_schema_lists() {
    // `false` and `"off"` are a stated opinion (do not deduplicate), which is
    // not the same as `"inherit"` stating none.
    for (input, expected) in [
        (r#"{"dedup":true}"#, Some(StringDedup::Block)),
        (r#"{"dedup":false}"#, Some(StringDedup::Off)),
        (r#"{"dedup":"inherit"}"#, None),
        (r#"{"dedup":"off"}"#, Some(StringDedup::Off)),
        (r#"{"dedup":"exact"}"#, Some(StringDedup::Exact)),
        (r#"{"dedup":"block"}"#, Some(StringDedup::Block)),
        (r#"{"dedup":"contains"}"#, Some(StringDedup::Contains)),
        (r"{}", None),
    ] {
        let parsed: PartialMergedString = serde_json::from_str(input).unwrap();

        assert_eq!(parsed.dedup, expected, "parsing {input}");
    }
}
