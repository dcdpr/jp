//! Map types with configurable merge strategies.
//!
//! Mirrors the [`MergeableVec`](super::vec::MergeableVec) pattern for
//! `IndexMap<String, T>` values. Used by [`JsonValue`](super::json_value::JsonValue)
//! for object-typed values, and can be used directly in typed config fields.

use std::ops::{Deref, DerefMut};

use indexmap::IndexMap;
use schematic::{Config, ConfigEnum, PartialConfig as _, Schematic};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_untagged::UntaggedEnumVisitor;

use crate::{delta::PartialConfigDelta, partial::ToPartial};

/// Map of `String` to `T`, either defaulting to a merge strategy of
/// `deep_merge`, or defining a specific merge strategy.
///
/// This should be used in combination with
/// [`crate::internal::merge::map_with_strategy`].
///
/// The name ends in `Map` so schematic's `Config` macro treats it as a
/// container type, similar to how `MergeableVec` ends in `Vec`.
#[derive(Debug, Clone, PartialEq, Serialize, Config)]
#[serde(untagged, rename_all = "snake_case")]
pub enum MergeableMap<T> {
    /// A map merged using the default strategy (deep merge).
    #[setting(default)]
    Map(IndexMap<String, T>),

    /// A map merged using an explicit strategy.
    #[setting(nested)]
    Merged(MergedMap<T>),
}

impl<T> Deref for MergeableMap<T> {
    type Target = IndexMap<String, T>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Map(v) => v,
            Self::Merged(v) => &v.value,
        }
    }
}

impl<T> DerefMut for MergeableMap<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Map(v) => v,
            Self::Merged(v) => &mut v.value,
        }
    }
}

impl<'de, T> Deserialize<'de> for MergeableMap<T>
where
    T: Clone + DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Try as `MergedMap` first (has `value` + `strategy` keys), then
        // fall back to a plain map.
        UntaggedEnumVisitor::new()
            .map(|map| {
                let value: serde_json::Value = map.deserialize()?;

                // Peek: does this look like a MergedMap?
                if let Some(obj) = value.as_object()
                    && obj.contains_key("value")
                    && obj.contains_key("strategy")
                    && let Ok(merged) = serde_json::from_value::<MergedMap<T>>(value.clone())
                {
                    return Ok(Self::Merged(merged));
                }

                // Plain map.
                serde_json::from_value(value)
                    .map(Self::Map)
                    .map_err(serde::de::Error::custom)
            })
            .deserialize(deserializer)
    }
}

impl<T> MergeableMap<T> {
    /// Consumes the `MergeableMap` and returns the underlying `IndexMap`.
    #[must_use]
    pub fn into_map(self) -> IndexMap<String, T> {
        match self {
            Self::Map(v) => v,
            Self::Merged(v) => v.value,
        }
    }

    /// Returns `true` if the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Map(v) => v.is_empty(),
            Self::Merged(v) => v.value.is_empty(),
        }
    }

    /// Returns `true` if this is a discardable default value.
    #[must_use]
    pub const fn discard_when_merged(&self) -> bool {
        matches!(self, Self::Merged(v) if v.discard_when_merged)
    }
}

impl<T> Default for MergeableMap<T> {
    fn default() -> Self {
        Self::Map(IndexMap::default())
    }
}

impl<T> From<IndexMap<String, T>> for MergeableMap<T> {
    fn from(value: IndexMap<String, T>) -> Self {
        Self::Map(value)
    }
}

impl<T> From<MergeableMap<T>> for IndexMap<String, T> {
    fn from(value: MergeableMap<T>) -> Self {
        match value {
            MergeableMap::Map(v) => v,
            MergeableMap::Merged(v) => v.value,
        }
    }
}

impl<T: Config + Clone + PartialEq + Serialize + DeserializeOwned + ToPartial>
    ToPartial<MergeableMap<T::Partial>> for MergeableMap<T>
{
    fn to_partial(&self) -> MergeableMap<T::Partial> {
        // Always emit `Merged` with `Replace` strategy to avoid re-applying
        // the original strategy on subsequent merges.
        let value = match self {
            Self::Map(v) | Self::Merged(MergedMap { value: v, .. }) => {
                v.iter().map(|(k, v)| (k.clone(), v.to_partial())).collect()
            }
        };

        MergeableMap::Merged(MergedMap {
            value,
            strategy: Some(MergedMapStrategy::Replace),
            discard_when_merged: false,
        })
    }
}

impl<T> PartialConfigDelta for PartialMergeableMap<T>
where
    T: Default + PartialEq + Clone + Serialize + DeserializeOwned + Schematic,
{
    fn delta(&self, next: Self) -> Self {
        if self == &next {
            return Self::empty();
        }
        next
    }
}

/// A map with an explicit merge strategy.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(rename_all = "snake_case")]
pub struct MergedMap<T> {
    /// The map value.
    #[setting(default)]
    pub value: IndexMap<String, T>,

    /// The merge strategy.
    ///
    /// - `deep_merge`: Recursive per-key merge (default).
    /// - `merge`: Shallow merge (top-level keys from next win, no recursion).
    /// - `keep`: Only insert keys absent from the base.
    /// - `replace`: Replace the entire map.
    #[setting(default, skip_serializing_if = "Option::is_none")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<MergedMapStrategy>,

    /// Whether the value is discarded when another value is merged in.
    #[setting(default)]
    #[serde(default)]
    pub discard_when_merged: bool,
}

impl<T> From<MergedMap<T>> for MergeableMap<T> {
    fn from(value: MergedMap<T>) -> Self {
        Self::Merged(value)
    }
}

impl<T> ToPartial for MergedMap<T>
where
    T: Default + Clone + PartialEq + Serialize + DeserializeOwned + Schematic,
{
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            value: Some(self.value.clone()),
            strategy: self.strategy,
            discard_when_merged: Some(self.discard_when_merged),
        }
    }
}

/// Merge strategy for `MergeableMap`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MergedMapStrategy {
    /// Recursive per-key merge. Nested objects are merged recursively.
    #[default]
    DeepMerge,

    /// Shallow merge. Top-level keys from next win, but nested objects are
    /// replaced rather than recursed into.
    Merge,

    /// Only insert keys absent from the base. Existing keys are never
    /// overwritten.
    Keep,

    /// Replace the entire map.
    Replace,
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
