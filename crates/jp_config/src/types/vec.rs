//! Vec types.

use std::ops::{Deref, DerefMut};

use schematic::{Config, ConfigEnum, PartialConfig as _, Schematic};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_untagged::UntaggedEnumVisitor;

use crate::{delta::PartialConfigDelta, partial::ToPartial};

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
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        match self {
            Self::Vec(v) => v.is_empty(),
            Self::Merged(v) => v.value.is_empty(),
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
}

impl<T: Config + Clone + PartialEq + Serialize + DeserializeOwned + ToPartial>
    ToPartial<MergeableVec<T::Partial>> for MergeableVec<T>
{
    fn to_partial(&self) -> MergeableVec<T::Partial> {
        match self {
            Self::Vec(v) => MergeableVec::Vec(v.iter().map(ToPartial::to_partial).collect()),
            Self::Merged(v) => MergeableVec::Merged(MergedVec {
                value: v.value.iter().map(ToPartial::to_partial).collect(),
                strategy: v.strategy,
                discard_when_merged: v.discard_when_merged,
            }),
        }
    }
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

/// Strings that are merged using the specified merge strategy.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(rename_all = "snake_case")]
pub struct MergedVec<T> {
    /// The vec value.
    #[setting(default = vec![])]
    pub value: Vec<T>,

    /// The merge strategy.
    ///
    /// - `append`: Append the vec to the previous value.
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

    /// See [`schematic::merge::replace`].
    Replace,
}
