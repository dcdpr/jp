//! Extended configuration types.

pub mod byte_size;
pub mod color;
pub mod command;
pub mod extending_path;
pub mod json_value;
pub mod map;
pub mod policy_spec;
pub mod string;
pub mod vec;

use std::str::FromStr;

use serde::de::{Deserializer, Error as DeError, Visitor};

use crate::BoxedError;

/// The three states a `dedup` setting can be written in.
///
/// `Inherit` carries no opinion, leaving the merge strategies to inherit one
/// from the previous layer.
/// Used to parse `dedup` from key-value assignments (`--cfg …dedup=inherit`),
/// mirroring the values [`deserialize_dedup`] accepts from config files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dedup {
    /// `true`
    Enabled,

    /// `false`
    Disabled,

    /// `"inherit"`
    Inherit,
}

impl Dedup {
    /// The opinion this state carries, if any.
    pub(crate) const fn opinion(self) -> Option<bool> {
        match self {
            Self::Enabled => Some(true),
            Self::Disabled => Some(false),
            Self::Inherit => None,
        }
    }
}

impl From<bool> for Dedup {
    fn from(v: bool) -> Self {
        if v { Self::Enabled } else { Self::Disabled }
    }
}

impl FromStr for Dedup {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "true" => Ok(Self::Enabled),
            "false" => Ok(Self::Disabled),
            "inherit" => Ok(Self::Inherit),
            _ => Err(format!("expected `true`, `false` or `inherit`, got `{s}`").into()),
        }
    }
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
