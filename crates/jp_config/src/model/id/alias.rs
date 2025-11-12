//! Serializer and deserializer for model aliases.

use serde::{Deserialize as _, Deserializer, Serialize as _, Serializer};

/// Serialize an alias as a string.
pub fn serialize<S>(alias: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if alias.is_empty() || alias.chars().any(|c| c == '/') {
        return Err(serde::ser::Error::custom(
            "Alias must not be empty and must not contain '/'.",
        ));
    }

    alias.serialize(serializer)
}

/// Deserialize an alias from a string.
pub fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let alias = String::deserialize(deserializer)?;
    if alias.is_empty() || alias.chars().any(|c| c == '/') {
        return Err(serde::de::Error::custom(
            "Alias must not be empty and must not contain '/'.",
        ));
    }

    Ok(alias)
}
