//! String types.

use std::{convert::Infallible, ops::Deref, str::FromStr};

use schematic::{Config, ConfigEnum, PartialConfig as _};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::PartialConfigDelta,
    partial::ToPartial,
};

/// String value, either defaulting to a merge strategy of `replace`, or
/// defining a specific merge strategy.
#[derive(Debug, Clone, Config)]
#[config(serde(untagged))]
pub enum StringWithMerge {
    /// A string that is merged using the [`schematic::merge::replace`]
    #[setting(default)]
    String(String),

    /// A string that is merged using the specified merge strategy.
    #[setting(nested)]
    Merged(StringWithStrategy),
}

impl From<&str> for PartialStringWithMerge {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl FromStr for PartialStringWithMerge {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::String(s.to_owned()))
    }
}

impl From<StringWithMerge> for String {
    fn from(value: StringWithMerge) -> Self {
        match value {
            StringWithMerge::String(v) => v,
            StringWithMerge::Merged(v) => v.value,
        }
    }
}

impl AsRef<str> for PartialStringWithMerge {
    fn as_ref(&self) -> &str {
        match self {
            Self::String(v) => v,
            Self::Merged(v) => v.value.as_deref().unwrap_or_default(),
        }
    }
}

impl Deref for PartialStringWithMerge {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl AssignKeyValue for PartialStringWithMerge {
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

impl PartialConfigDelta for PartialStringWithMerge {
    fn delta(&self, next: Self) -> Self {
        if self == &next {
            return Self::empty();
        }

        next
    }
}

impl ToPartial for StringWithMerge {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::String(v) => Self::Partial::String(v.clone()),
            Self::Merged(v) => Self::Partial::Merged(v.to_partial()),
        }
    }
}

/// Strings that are merged using the specified merge strategy.
#[derive(Debug, Clone, PartialEq, Config)]
pub struct StringWithStrategy {
    /// The string value.
    #[setting(default)]
    pub value: String,

    /// The merge strategy.
    #[setting(default)]
    pub strategy: StringMergeStrategy,
}

impl AssignKeyValue for PartialStringWithStrategy {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "value" => self.value = kv.try_some_string()?,
            "strategy" => self.strategy = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl ToPartial for StringWithStrategy {
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
pub enum StringMergeStrategy {
    /// Append the string to the previous value, without any separator.
    #[default]
    Append,

    /// Append the string to the previous value, with a space separator.
    AppendSpace,

    /// Append the string to the previous value, with a new line separator.
    AppendLine,

    /// Append the string to the previous value, with two new line separators.
    AppendParagraph,

    /// See [`schematic::merge::replace`].
    Replace,
}
