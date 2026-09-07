use schematic::{
    Schema,
    schema::{
        ArrayType, BooleanType, EnumType, LiteralValue, ObjectType, SchemaField, StringType,
        StructType, UnionType,
    },
};

use super::render;

#[test]
fn renders_a_scalar_on_one_line() {
    let schema = Schema::boolean(BooleanType::default());

    assert_eq!(render(&schema), "bool\n");
}

#[test]
fn renders_struct_fields_one_per_line() {
    let schema = Schema::structure(StructType::new([
        ("name".to_owned(), Schema::string(StringType::default())),
        (
            "enabled".to_owned(),
            Schema::boolean(BooleanType::default()),
        ),
    ]));

    // Fields come from a `BTreeMap`, so they sort by name rather than by
    // declaration order.
    assert_eq!(render(&schema), "enabled: bool\nname: string\n");
}

#[test]
fn marks_an_optional_field() {
    let mut field = SchemaField::new(Schema::string(StringType::default()));
    field.optional = true;

    let schema = Schema::structure(StructType::new([("name".to_owned(), field)]));

    assert_eq!(render(&schema), "name?: string\n");
}

#[test]
fn renders_an_enum_with_its_accepted_values() {
    let schema = Schema::enumerable(EnumType::new([
        LiteralValue::String("off".to_owned()),
        LiteralValue::String("full".to_owned()),
    ]));

    assert_eq!(render(&schema), "\"off\" | \"full\"\n");
}

#[test]
fn renders_variant_aliases_beside_the_canonical_value() {
    let mut variant = SchemaField::new(Schema::literal_value(LiteralValue::String(
        "strip-responses".to_owned(),
    )));
    variant.aliases = vec!["strip_responses".to_owned(), "sres".to_owned()];

    let schema = Schema::enumerable(EnumType::from_fields(
        [("StripResponses".to_owned(), variant)],
        None,
    ));

    assert_eq!(
        render(&schema),
        "\"strip-responses\" (aka strip_responses, sres)\n"
    );
}

#[test]
fn renders_a_string_pattern() {
    let schema = Schema::string(StringType {
        pattern: Some("^[a-z]+$".to_owned()),
        ..StringType::default()
    });

    assert_eq!(render(&schema), "string(/^[a-z]+$/)\n");
}

#[test]
fn renders_an_array_of_scalars_inline() {
    let schema = Schema::array(ArrayType::new(Schema::string(StringType::default())));

    assert_eq!(render(&schema), "[string]\n");
}

#[test]
fn opens_a_block_for_an_array_of_structs() {
    let item = Schema::structure(StructType::new([(
        "uri".to_owned(),
        Schema::string(StringType::default()),
    )]));

    let schema = Schema::structure(StructType::new([(
        "attachments".to_owned(),
        Schema::array(ArrayType::new(item)),
    )]));

    assert_eq!(render(&schema), "attachments:\n  []:\n    uri: string\n");
}

#[test]
fn labels_a_map_value_with_a_star() {
    let schema = Schema::object(ObjectType::new(
        Schema::string(StringType::default()),
        Schema::structure(StructType::new([(
            "enabled".to_owned(),
            Schema::boolean(BooleanType::default()),
        )])),
    ));

    assert_eq!(render(&schema), "*:\n  enabled: bool\n");
}

#[test]
fn renders_a_union_of_scalars_inline() {
    let schema = Schema::union(UnionType::new_any([
        Schema::boolean(BooleanType::default()),
        Schema::null(),
    ]));

    assert_eq!(render(&schema), "bool | null\n");
}

#[test]
fn marks_the_expanded_variant_of_a_union() {
    let table = Schema::structure(StructType::new([(
        "state".to_owned(),
        Schema::boolean(BooleanType::default()),
    )]));

    let schema = Schema::union(
        UnionType::new_any([Schema::boolean(BooleanType::default()), table]).with_expanded_index(1),
    );

    assert_eq!(render(&schema), "|: bool\n| (expanded):\n  state: bool\n");
}

/// A named type that encloses itself is emitted as a reference by the schema
/// builder, and the renderer has to stop there or it would not terminate.
#[test]
fn collapses_a_type_that_encloses_itself() {
    let mut inner = Schema::structure(StructType::new([(
        "child".to_owned(),
        Schema::structure(StructType::default()),
    )]));
    inner.name = Some("Node".to_owned());

    // Stand in for what `SchemaBuilder::infer` produces on the second visit.
    let mut outer = Schema::structure(StructType::new([("child".to_owned(), inner)]));
    outer.name = Some("Node".to_owned());

    let schema = Schema::structure(StructType::new([("root".to_owned(), outer)]));

    assert_eq!(render(&schema), "root: Node\n  child: @Node\n");
}
