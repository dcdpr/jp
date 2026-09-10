//! The shape `#[derive(Schematic)]` gives an enum's variants.
//!
//! An enum mixing unit and payload variants is described as a union, one
//! variant at a time, and each variant has to be described in the shape serde
//! writes it in rather than in the shape the Rust declaration suggests.

use schematic::{
    Schema, SchemaBuilder, SchemaType, Schematic,
    schema::{LiteralValue, UnionType},
};

/// An enum mixing a unit variant with a payload variant, on serde's default
/// (externally tagged) representation.
#[derive(Default, Schematic)]
#[expect(dead_code, reason = "only the generated schema is under test")]
enum Target {
    /// The whole tree.
    #[default]
    Everything,

    /// A named entry.
    Named,

    /// Reserved for internal use, and not part of the wire vocabulary.
    #[schema(skip)]
    Internal,

    /// One path.
    Path(String),
}

/// An enum whose variants are all units, which is described as an enumeration
/// rather than a union.
#[derive(Schematic)]
#[expect(dead_code, reason = "only the generated schema is under test")]
enum Mode {
    Fast,
    Slow,
}

fn union_of<T: Schematic>() -> UnionType {
    let schema = SchemaBuilder::build_root::<T>();

    match schema.ty {
        SchemaType::Union(union) => *union,
        other => panic!("expected a union, got {other:?}"),
    }
}

fn literal_of(schema: &Schema) -> Option<&str> {
    match &schema.ty {
        SchemaType::Literal(literal) => match &literal.value {
            LiteralValue::String(value) => Some(value),
            _ => None,
        },
        _ => None,
    }
}

/// A unit variant is written as the bare variant name, so it is described as
/// that string and not as a single-key table holding it.
#[test]
fn a_unit_variant_is_described_as_its_own_name() {
    let union = union_of::<Target>();
    let names: Vec<_> = union
        .variants_types
        .iter()
        .filter_map(|variant| literal_of(variant))
        .collect();

    assert_eq!(names, ["everything", "named"]);
}

/// A payload variant keeps the wrapper serde writes around it.
#[test]
fn a_payload_variant_keeps_its_tag() {
    let union = union_of::<Target>();
    let tagged: Vec<_> = union
        .variants_types
        .iter()
        .filter_map(|variant| match &variant.ty {
            SchemaType::Struct(inner) => Some(inner.fields.keys().cloned().collect::<Vec<_>>()),
            _ => None,
        })
        .collect();

    assert_eq!(tagged, [["path".to_owned()]]);
}

/// A skipped variant is not input a user may write, so it is left out.
#[test]
fn a_skipped_variant_is_left_out() {
    let union = union_of::<Target>();

    assert_eq!(
        union.variants_types.len(),
        3,
        "two unit variants and one payload variant, with the skipped one gone"
    );
}

/// `#[default]` names the same thing to a schema reader as
/// `#[setting(default)]` does, so the derive reads both.
#[test]
fn the_derive_default_attribute_sets_the_default_index() {
    let union = union_of::<Target>();

    assert_eq!(union.default_index, Some(0));
}

#[test]
fn an_all_unit_enum_is_described_as_an_enumeration() {
    let schema = SchemaBuilder::build_root::<Mode>();

    let SchemaType::Enum(enum_type) = schema.ty else {
        panic!("expected an enumeration");
    };

    assert_eq!(enum_type.values, [
        LiteralValue::String("fast".to_owned()),
        LiteralValue::String("slow".to_owned()),
    ]);
}
