//! String types.

use std::{convert::Infallible, ops::Deref, str::FromStr};

use schematic::{Config, ConfigEnum, PartialConfig as _};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    partial::ToPartial,
};

/// String value, either defaulting to a merge strategy of `replace`, or
/// defining a specific merge strategy.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(serde(untagged))]
pub enum MergeableString {
    /// A string that is merged using the [`schematic::merge::replace`]
    #[setting(default)]
    String(String),

    /// A string that is merged using the specified merge strategy.
    #[setting(nested)]
    Merged(MergedString),
}

impl PartialMergeableString {
    /// Returns `true` if the `MergeableString` is the default value.
    #[must_use]
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Merged(v) if v.is_default.is_some_and(|v| v))
    }
}

impl From<&str> for PartialMergeableString {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl FromStr for PartialMergeableString {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::String(s.to_owned()))
    }
}

impl From<MergeableString> for String {
    fn from(value: MergeableString) -> Self {
        match value {
            MergeableString::String(v) => v,
            MergeableString::Merged(v) => v.value,
        }
    }
}

impl AsRef<str> for PartialMergeableString {
    fn as_ref(&self) -> &str {
        match self {
            Self::String(v) => v,
            Self::Merged(v) => v.value.as_deref().unwrap_or_default(),
        }
    }
}

impl Deref for PartialMergeableString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl AssignKeyValue for PartialMergeableString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::String(_) => return missing_key(&kv),
                Self::Merged(config) => config.assign(kv)?,
            },
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialMergeableString {
    fn delta(&self, next: Self) -> Self {
        if self == &next {
            return Self::empty();
        }

        next
    }
}

impl ToPartial for MergeableString {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::String(v) => Self::Partial::String(v.clone()),
            Self::Merged(v) => Self::Partial::Merged(v.to_partial()),
        }
    }
}

/// Strings that are merged using the specified merge strategy.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct MergedString {
    /// The string value.
    #[setting(default)]
    pub value: String,

    /// The merge strategy.
    #[setting(default)]
    pub strategy: MergedStringStrategy,

    /// The separator to use between the previous value and the new value.
    #[setting(default)]
    pub separator: MergedStringSeparator,

    /// Whether the value is the default value.
    ///
    /// When `true`, if another value is merged in, this value will be
    /// overwritten.
    #[setting(default)]
    pub is_default: bool,
}

impl AssignKeyValue for PartialMergedString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "value" => self.value = kv.try_some_string()?,
            "strategy" => self.strategy = kv.try_some_from_str()?,
            "separator" => self.separator = kv.try_some_from_str()?,
            "is_default" => self.is_default = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl ToPartial for MergedString {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            value: Some(self.value.clone()),
            strategy: Some(self.strategy),
            separator: Some(self.separator),
            is_default: Some(self.is_default),
        }
    }
}

/// Merge strategy for `MergeableString`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MergedStringStrategy {
    /// Append the string to the previous value.
    #[default]
    Append,

    /// Prepend the string to the previous value.
    Prepend,

    /// See [`schematic::merge::replace`].
    Replace,
}

/// Merge strategy for `VecWithStrategy`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum MergedStringSeparator {
    /// No separator.
    #[default]
    None,

    /// Single space separator.
    Space,

    /// New line separator.
    Line,

    /// Paragraph separator.
    Paragraph,
}

impl MergedStringSeparator {
    /// Returns the separator as a string.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::None => "",
            Self::Space => " ",
            Self::Line => "\n",
            Self::Paragraph => "\n\n",
        }
    }
}
