//! Retired spellings of a `ConfigEnum` variant.
//!
//! A deprecated alias keeps old input parsing, and stays out of everything the
//! type advertises, so nothing offers it to a new user.

use std::str::FromStr as _;

use schematic::{
    ConfigEnum, SchemaBuilder, SchemaType,
    schema::{LiteralValue, SchemaField},
};

#[derive(Debug, Clone, Copy, PartialEq, ConfigEnum)]
#[config(rename_all = "lowercase")]
enum Policy {
    Ask,

    #[variant(aliases("yes"), deprecated_aliases("unattended"))]
    Allow,
}

#[test]
fn a_deprecated_alias_parses_as_its_variant() {
    assert_eq!(Policy::from_str("unattended").unwrap(), Policy::Allow);
}

#[test]
fn a_deprecated_alias_parses_every_time() {
    // The warning is logged once per process; the parse must not depend on it.
    assert_eq!(Policy::from_str("unattended").unwrap(), Policy::Allow);
    assert_eq!(Policy::from_str("unattended").unwrap(), Policy::Allow);
}

#[test]
fn a_variant_parsed_from_a_deprecated_alias_displays_its_canonical_spelling() {
    assert_eq!(Policy::from_str("unattended").unwrap().to_string(), "allow");
}

#[test]
fn a_deprecated_alias_is_not_offered_by_the_schema() {
    let schema = SchemaBuilder::build_root::<Policy>();
    let SchemaType::Enum(enum_type) = schema.ty else {
        panic!("expected an enumeration");
    };

    assert_eq!(enum_type.values, [
        LiteralValue::String("ask".to_owned()),
        LiteralValue::String("allow".to_owned()),
    ]);

    let aliases: Vec<&SchemaField> = enum_type
        .variants
        .as_ref()
        .expect("variants")
        .values()
        .map(AsRef::as_ref)
        .collect();

    assert_eq!(aliases[1].aliases, ["yes"]);
}
