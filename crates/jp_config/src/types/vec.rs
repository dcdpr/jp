//! Vec types.

use std::ops::{Deref, DerefMut};

use schematic::{Config, ConfigEnum, PartialConfig as _, Schematic};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

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
#[derive(Debug, Config, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum MergeableVec<T> {
    /// A vec that is merged using the [`schematic::merge::replace`]
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
}

impl<T: ToPartial> MergeableVec<T> {
    /// Convert the `MergeableVec<T>` into a [`MergeableVec`] of `T:Partial`.
    ///
    /// Note that we do not implement `ToPartial` for `MergeableVec<T>` because
    /// that trait requires `to_partial` to return a `PartialConfig`, which is
    /// not what we want in this case.
    ///
    /// For more details, see [`MergeableVec`].
    #[must_use]
    pub fn to_partial(&self) -> MergeableVec<T::Partial> {
        match self {
            Self::Vec(v) => MergeableVec::Vec(v.iter().map(ToPartial::to_partial).collect()),
            Self::Merged(v) => MergeableVec::Merged(MergedVec {
                value: v.value.iter().map(ToPartial::to_partial).collect(),
                strategy: v.strategy,
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
#[derive(Debug, Config, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergedVec<T> {
    /// The vec value.
    #[setting(default = vec![])]
    pub value: Vec<T>,

    /// The merge strategy.
    #[setting(default)]
    pub strategy: MergedVecStrategy,
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
            strategy: Some(self.strategy),
        }
    }
}

/// Merge strategy for `VecWithStrategy`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MergedVecStrategy {
    /// Append the string to the previous value, without any separator.
    #[default]
    Append,

    /// See [`schematic::merge::replace`].
    Replace,
}
