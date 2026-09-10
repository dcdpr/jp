//! Schema attributes on individual config fields.

use schematic::{
    Config, Schema, SchemaBuilder, SchemaType,
    schema::{BooleanType, LiteralValue, UnionType},
};

/// A literal default alongside an additional input shape.
#[derive(Config)]
#[expect(dead_code, reason = "only the generated schema is under test")]
struct DefaultedInput {
    #[setting(default = true, schema_union_with = inherit_shape)]
    enabled: bool,
}

fn inherit_shape(_: &SchemaBuilder) -> Vec<Schema> {
    vec![Schema::literal_value(LiteralValue::String(
        "inherit".to_owned(),
    ))]
}

#[test]
fn a_literal_default_preserves_additional_input_shapes() {
    let schema = SchemaBuilder::build_root::<DefaultedInput>();
    let SchemaType::Struct(fields) = schema.ty else {
        panic!("expected a struct");
    };
    let field = &fields.fields["enabled"];

    assert!(field.optional);
    assert_eq!(
        field.schema,
        Schema::union(
            UnionType::new_any([
                Schema::literal_value(LiteralValue::String("inherit".to_owned())),
                Schema::boolean(BooleanType::new(true)),
            ])
            .with_expanded_index(1)
        )
    );
}
