pub mod global_id;
pub mod target_id;
pub mod variant;

use std::str::FromStr;

pub use global_id::GlobalId;
pub use target_id::TargetId;
pub use variant::Variant;

use crate::error::Error;

pub struct Parts {
    pub prefix: String,
    pub variant: Variant,
    pub target_id: TargetId,
    pub global_id: GlobalId,
}

impl Parts {
    /// Parse the given string as a [`Parts`], with the given variant expected.
    pub fn parse_with_variant(s: &str, variant: impl Into<Variant>) -> Result<Self, Error> {
        let variant = variant.into();

        let parts = Parts::from_str(s)?;
        if parts.variant != variant {
            return Err(Error::UnexpectedVariant(*variant, *parts.variant));
        }

        Ok(parts)
    }
}

impl FromStr for Parts {
    type Err = Error;

    // `<prefix>-<variant><target_id>-<global_id>`
    fn from_str(s: &str) -> Result<Self, Error> {
        // `(<prefix>, <variant><target_id>-<global_id>)`
        let (prefix, s) = s
            .split_once('-')
            .map(|(p, s)| (p.to_owned(), s))
            .ok_or(Error::MissingPrefix(s.to_owned()))?;

        if prefix != super::ID_PREFIX {
            return Err(Error::InvalidPrefix(super::ID_PREFIX, prefix));
        }

        // `(<variant><target_id>, <global_id>)`
        let (mut variant_with_target_id, global_id) = s
            .split_once('-')
            .ok_or(Error::MissingVariantAndTargetId)
            .map(|(v, s)| (v.chars(), GlobalId::new(s.to_owned())))?;

        let variant = Variant::new(variant_with_target_id.next().ok_or(Error::MissingVariant)?);
        if !variant.is_valid() {
            return Err(Error::InvalidVariant(*variant));
        }

        let target_id = TargetId::new(variant_with_target_id.collect::<String>());
        if target_id.is_empty() {
            return Err(Error::MissingTargetId);
        }

        if global_id.is_empty() {
            return Err(Error::MissingGlobalId);
        }

        if global_id
            .chars()
            .any(|c| !(c.is_numeric() || (c.is_ascii_alphabetic() && c.is_ascii_lowercase())))
        {
            return Err(Error::InvalidGlobalId(global_id.to_string()));
        }

        Ok(Self {
            prefix,
            variant,
            target_id,
            global_id,
        })
    }
}
