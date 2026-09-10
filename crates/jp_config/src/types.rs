//! Extended configuration types.

pub mod api_key_env;
pub mod byte_size;
pub mod color;
pub mod command;
pub mod extending_path;
pub mod json_value;
pub mod map;
pub mod policy_spec;
pub mod string;
pub mod vec;

use schematic::{
    Schema, SchemaBuilder,
    schema::{EnumType, LiteralValue},
};
use serde::de::{Deserializer, Error as DeError, Visitor};

/// The non-boolean shapes a `dedup` field accepts, for the schema.
///
/// The boolean is described by the field's own type; these are the strings
/// [`deserialize_dedup`] also takes, which the field type cannot express.
pub(crate) fn dedup_input_shapes(schema: &SchemaBuilder) -> Vec<Schema> {
    vec![schema.nest().enumerable(EnumType::new([
        LiteralValue::String("inherit".into()),
        LiteralValue::String("true".into()),
        LiteralValue::String("false".into()),
    ]))]
}

/// Deserialize a `dedup` field from `true`, `false`, or `"inherit"`.
///
/// `"inherit"` and an absent field both produce `None`, which the merge
/// strategies read as "no opinion" and inherit from the previous layer.
pub(crate) fn deserialize_dedup<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    struct DedupVisitor;

    impl Visitor<'_> for DedupVisitor {
        type Value = Option<bool>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a boolean or \"inherit\"")
        }

        fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            match v {
                "inherit" => Ok(None),
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                _ => Err(DeError::unknown_variant(v, &["true", "false", "inherit"])),
            }
        }

        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(DedupVisitor)
}
