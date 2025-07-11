#![allow(clippy::ref_option)]

use confique::Partial;
use serde::Deserialize as _;

pub(crate) fn is_nested_default_or_empty<T: Partial + PartialEq>(v: &T) -> bool {
    is_nested_default(v) || is_nested_empty(v)
}

pub(crate) fn is_nested_default<T: Partial + PartialEq>(v: &T) -> bool {
    v == &T::default_values()
}

pub(crate) fn is_nested_empty<T: Partial + PartialEq>(v: &T) -> bool {
    v == &T::empty()
}

pub(crate) fn is_none_or_default<T: Default + PartialEq>(v: &Option<T>) -> bool {
    v.as_ref().is_none_or(|v| v == &T::default())
}

pub(crate) fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    v == &T::default()
}

pub(crate) fn de_from_str<'de, D, T, E>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr<Err = E>,
    E: std::error::Error,
{
    String::deserialize(deserializer)
        .and_then(|v| T::from_str(&v).map_err(serde::de::Error::custom))
}

pub(crate) fn de_from_str_opt<'de, D, T, E>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr<Err = E>,
    E: std::error::Error,
{
    Option::<String>::deserialize(deserializer)?
        .map(|v| T::from_str(&v))
        .transpose()
        .map_err(serde::de::Error::custom)
}
