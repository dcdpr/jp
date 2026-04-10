//! Vec types.

use std::ops::{Deref, DerefMut};

use schematic::{Config, ConfigEnum, PartialConfig as _, Schematic};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_untagged::UntaggedEnumVisitor;

use crate::{delta::PartialConfigDelta, fill::FillDefaults, partial::ToPartial};

/// Vec of `T`'s, either defaulting to a merge strategy of `replace`, or
/// defining a specific merge strategy.
///
/// This should be used in combination with
/// [`crate::internal::merge::vec_with_strategy`]
///
/// There are some nuances to this type that are worth noting:
///
/// - The name of the type is important, it *has* to end in `Vec` for the
///   [`schematic::Config`] macro to consider the type as a "container" type,
///   allowing it to be treated similar to regular [`Vec`]s.
///
/// - When used in other types that implement [`schematic::Config`], the
///   [`Config::Partial`] associated type of that type will have the regular
///   `MergeableVec` type for the relevant field *NOT* the
///   [`PartialMergeableVec`] type. This is inline with how `schematic` works
///   for other container types such as `Vec` and `HashMap`.
///
/// - At this moment, this type does *not* implement [`AssignKeyValue`], unlike
///   e.g. [`MergeableString`]. This is because the generic `T` means that it is
///   not immediately clear how we would parse the provided value; would we
///   parse it as [`MergedVec`] or as `T`? This can be changed later, if needed.
///
/// [`AssignKeyValue`]: crate::AssignKeyValue
/// [`MergeableString`]: super::string::MergeableString
#[derive(Debug, Clone, PartialEq, Serialize, Config)]
#[serde(untagged, rename_all = "snake_case")]
pub enum MergeableVec<T> {
    /// A vec that is merged using the [`schematic::merge::append_vec`]
    #[setting(default)]
    Vec(Vec<T>),
    /// A vec that is merged using the specified merge strategy.
    #[setting(nested)]
    Merged(MergedVec<T>),
}

impl<T> Deref for MergeableVec<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Vec(v) => v,
            Self::Merged(v) => &v.value,
        }
    }
}

impl<T> DerefMut for MergeableVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Vec(v) => v,
            Self::Merged(v) => &mut v.value,
        }
    }
}

impl<'de, T> Deserialize<'de> for MergeableVec<T>
where
    T: Clone + DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        UntaggedEnumVisitor::new()
            .seq(|v| v.deserialize().map(MergeableVec::Vec))
            .map(|map| map.deserialize().map(MergeableVec::Merged))
            .deserialize(deserializer)
    }
}

impl<T> MergeableVec<T> {
    /// Consumes the `MergeableVec` and returns the underlying [`Vec`].
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Self::Vec(v) => v,
            Self::Merged(v) => v.value,
        }
    }

    /// Returns `true` if the `MergeableVec` is empty.
    ///
    /// A `Merged` variant with no items but with metadata (e.g. `dedup` or
    /// `strategy`) is NOT considered empty, because the metadata must still
    /// participate in merges.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        match self {
            Self::Vec(v) => v.is_empty(),
            Self::Merged(v) => {
                v.value.is_empty()
                    && v.strategy.is_none()
                    && v.dedup.is_none()
                    && !v.discard_when_merged
            }
        }
    }

    /// Returns `true` if the `MergeableVec` is the default value.
    #[must_use]
    pub const fn discard_when_merged(&self) -> bool {
        matches!(self, Self::Merged(v) if v.discard_when_merged)
    }

    /// Get a mutable reference to the [`MergedVec`], if applicable.
    pub const fn as_merged_mut(&mut self) -> Option<&mut MergedVec<T>> {
        match self {
            Self::Vec(_) => None,
            Self::Merged(v) => Some(v),
        }
    }

    /// Returns `true` if dedup is explicitly enabled.
    #[must_use]
    pub const fn dedup(&self) -> bool {
        matches!(self, Self::Merged(v) if matches!(v.dedup, Some(true)))
    }
}

impl<T: Config + Clone + PartialEq + Serialize + DeserializeOwned + ToPartial>
    ToPartial<MergeableVec<T::Partial>> for MergeableVec<T>
{
    fn to_partial(&self) -> MergeableVec<T::Partial> {
        // Always emit `Merged` with `Replace` strategy. The finalized value
        // already reflects all prior merges (appends), so preserving the
        // original strategy would cause `vec_with_strategy` to re-apply it
        // when the partial is merged again (e.g. in
        // `apply_conversation_config`), duplicating vec entries.
        //
        // Unlike `MergeableString` where we can flatten to the `String`
        // variant (which uses replace semantics), the `Vec` variant here
        // defaults to *append* semantics in `vec_with_strategy`. So we must
        // explicitly use `Merged` with `strategy: Some(Replace)`.
        let value = match self {
            Self::Vec(v) | Self::Merged(MergedVec { value: v, .. }) => {
                v.iter().map(ToPartial::to_partial).collect()
            }
        };

        MergeableVec::Merged(MergedVec {
            value,
            strategy: Some(MergedVecStrategy::Replace),
            dedup: None,
            discard_when_merged: false,
        })
    }
}

/// Convert a `Vec<T>` to a `MergeableVec<T::Partial>` with replace strategy.
///
/// Used by `ToPartial` impls for fields with `#[setting(partial_via =
/// MergeableVec)]` where the finalized type is `Vec<T>` but the partial is
/// `MergeableVec<T::Partial>`.
pub fn vec_to_mergeable_partial<T: ToPartial>(items: &[T]) -> MergeableVec<T::Partial> {
    MergeableVec::Merged(MergedVec {
        value: items.iter().map(ToPartial::to_partial).collect(),
        strategy: Some(MergedVecStrategy::Replace),
        dedup: None,
        discard_when_merged: false,
    })
}

impl<T> FromIterator<T> for MergeableVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::Vec(iter.into_iter().collect())
    }
}

impl<T> IntoIterator for MergeableVec<T> {
    type Item = T;

    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<T> From<Vec<T>> for MergeableVec<T> {
    fn from(value: Vec<T>) -> Self {
        Self::Vec(value)
    }
}

impl<T> Default for MergeableVec<T> {
    fn default() -> Self {
        Self::Vec(Vec::default())
    }
}

impl<T> FillDefaults for MergeableVec<T> {
    /// Fill metadata (dedup, strategy) from defaults when self has none.
    ///
    /// Items are always kept from self — only the `Merged` wrapper metadata is
    /// inherited when self is a plain `Vec` (no metadata).
    fn fill_from(self, defaults: Self) -> Self {
        // Already has metadata — keep as-is.
        let Self::Vec(items) = self else {
            return self;
        };

        // Defaults have metadata to inherit.
        if let Self::Merged(default_merged) = defaults
            && (default_merged.dedup.is_some() || default_merged.strategy.is_some())
        {
            return Self::Merged(MergedVec {
                value: items,
                strategy: default_merged.strategy,
                dedup: default_merged.dedup,
                discard_when_merged: false,
            });
        }

        Self::Vec(items)
    }
}

impl<T> From<MergeableVec<T>> for Vec<T> {
    fn from(value: MergeableVec<T>) -> Self {
        match value {
            MergeableVec::Vec(v) => v,
            MergeableVec::Merged(v) => v.value,
        }
    }
}

impl<T> AsRef<[T]> for PartialMergeableVec<T>
where
    T: Default + Clone + PartialEq + Serialize + DeserializeOwned + Schematic,
{
    fn as_ref(&self) -> &[T] {
        match self {
            Self::Vec(v) => v,
            Self::Merged(v) => v.value.as_deref().unwrap_or_default(),
        }
    }
}

impl<T> Deref for PartialMergeableVec<T>
where
    T: Default + Clone + PartialEq + Serialize + DeserializeOwned + Schematic,
{
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T> PartialConfigDelta for PartialMergeableVec<T>
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

/// Merged vec with explicit merge strategy metadata.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(rename_all = "snake_case")]
pub struct MergedVec<T> {
    /// The vec value.
    #[setting(default = vec![])]
    #[serde(default = "Vec::new")]
    pub value: Vec<T>,

    /// The merge strategy.
    ///
    /// - `append`: Append the vec to the previous value.
    /// - `prepend`: Prepend the vec before the previous value.
    /// - `replace`: Replace the previous value with the new value.
    #[setting(default, skip_serializing_if = "Option::is_none")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // The strategy is wrapped in an `Option`, because otherwise
    // `PartialMergedVec` would always set `strategy` to the default value,
    // which is not what we want.  Anyway, this isn't great, and it's related to
    // the fact that `MergedVec` behaves "special" compared to `MergedString` in
    // `schematic`, which isn't great either, but it's wrapped in tests, so if
    // you change this you'll see what happens.
    pub strategy: Option<MergedVecStrategy>,

    /// Whether to remove duplicate items after merging.
    ///
    /// Accepts `true`, `false`, or `"inherit"`.
    ///
    /// When `true`, items already present in the merged result are skipped.
    /// Comparison uses `PartialEq`. Order is preserved (first occurrence wins).
    ///
    /// This flag is "sticky": once a non-discarded config in the merge chain
    /// sets it to `true`, all subsequent merges for this field will deduplicate
    /// — unless a later config explicitly sets it to `false`.
    ///
    /// `"inherit"` (or omitting the field) means "no opinion" — inherit from
    /// the previous merge.
    #[setting(default, skip_serializing_if = "Option::is_none")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_dedup"
    )]
    pub dedup: Option<bool>,

    /// Whether the value is discarded when another value is merged in,
    /// regardless of the merge strategy of the other value.
    ///
    /// This is useful for "default" values that should only be used when no
    /// other value is set.
    #[setting(default)]
    #[serde(default)]
    pub discard_when_merged: bool,
}

impl<T> From<MergedVec<T>> for MergeableVec<T> {
    fn from(value: MergedVec<T>) -> Self {
        Self::Merged(value)
    }
}

impl<T> ToPartial for MergedVec<T>
where
    T: Default + Clone + PartialEq + Serialize + DeserializeOwned + Schematic,
{
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            value: Some(self.value.clone()),
            strategy: self.strategy,
            dedup: self.dedup,
            discard_when_merged: Some(self.discard_when_merged),
        }
    }
}

/// Merge strategy for `MergeableVec`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MergedVecStrategy {
    /// Append the vec to the previous value.
    #[default]
    Append,

    /// Prepend the vec before the previous value.
    Prepend,

    /// Replace the previous value with the new value.
    Replace,
}

/// Deserialize `dedup` from `true`, `false`, or `"inherit"` → `Option<bool>`.
fn deserialize_dedup<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct DedupVisitor;

    impl serde::de::Visitor<'_> for DedupVisitor {
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
                _ => Err(serde::de::Error::unknown_variant(v, &[
                    "true", "false", "inherit",
                ])),
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
