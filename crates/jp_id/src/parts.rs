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
    pub global_id: Option<GlobalId>,
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

    // `<prefix>-<variant><target_id>[-<global_id>]`
    fn from_str(s: &str) -> Result<Self, Error> {
        // `(<prefix>, <variant><target_id>[-<global_id>])`
        let (prefix, s) = s
            .split_once('-')
            .map(|(p, s)| (p.to_owned(), s))
            .ok_or(Error::MissingPrefix(s.to_owned()))?;

        if prefix != super::ID_PREFIX {
            return Err(Error::InvalidPrefix(super::ID_PREFIX, prefix));
        }

        // `(<variant><target_id>, Option<global_id>)`
        let (variant_with_target_id, global_id) = match s.split_once('-') {
            Some((v, g)) => (v.chars(), Some(GlobalId::new(g.to_owned()))),
            None => (s.chars(), None),
        };

        let mut variant_with_target_id = variant_with_target_id;
        let variant = Variant::new(variant_with_target_id.next().ok_or(Error::MissingVariant)?);
        if !variant.is_valid() {
            return Err(Error::InvalidVariant(*variant));
        }

        let target_id = TargetId::new(variant_with_target_id.collect::<String>());
        if target_id.is_empty() {
            return Err(Error::MissingTargetId);
        }

        if let Some(ref global_id) = global_id {
            if global_id.is_empty() {
                return Err(Error::MissingGlobalId);
            }

            if global_id
                .chars()
                .any(|c| !(c.is_numeric() || (c.is_ascii_alphabetic() && c.is_ascii_lowercase())))
            {
                return Err(Error::InvalidGlobalId(global_id.to_string()));
            }
        }

        Ok(Self {
            prefix,
            variant,
            target_id,
            global_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    #[allow(clippy::type_complexity)]
    fn test_parts_from_str() {
        #[rustfmt::skip]
        let cases: Vec<(&str, Result<(&str, char, &str, Option<&str>), &str>)> = vec![
            // With global_id
            ("jp-bar-baz", Ok(("jp", 'b', "ar", Some("baz")))),
            ("jp-qux-ba1z23", Ok(("jp", 'q', "ux", Some("ba1z23")))),
            ("jp-boo_baa_bop-ba1z13", Ok(("jp", 'b', "oo_baa_bop", Some("ba1z13")))),
            // Without global_id
            ("jp-bar", Ok(("jp", 'b', "ar", None))),
            ("jp-c17457886043", Ok(("jp", 'c', "17457886043", None))),
            // Errors
            ("jp", Err("Missing prefix: jp")),
            ("jp-", Err("Missing variant")),
            ("jp-b", Err("Missing target ID")),
            ("jp-foo-", Err("Missing global ID")),
            ("jp-afoo-baz-qux", Err("Invalid global ID, must be [a-z]: baz-qux")),
            ("foo-bar-baz", Err("Invalid prefix, must be jp: foo")),
        ];

        for (input, result) in cases {
            let parts = Parts::from_str(input)
                .map(|parts| {
                    (
                        parts.prefix,
                        parts.variant.into_inner(),
                        parts.target_id.to_string(),
                        parts.global_id.map(|g| g.to_string()),
                    )
                })
                .map_err(|e| e.to_string());

            let result = result
                .map(|(prefix, variant, target_id, global_id)| {
                    (
                        prefix.to_string(),
                        variant,
                        target_id.to_string(),
                        global_id.map(str::to_string),
                    )
                })
                .map_err(str::to_string);

            assert_eq!(parts, result, "input: {input}");
        }
    }
}
