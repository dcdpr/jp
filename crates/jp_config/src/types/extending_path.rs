//! `ExtendingRelativePath` type.

use std::{convert::Infallible, ops::Deref, str::FromStr};

use relative_path::{RelativePath, RelativePathBuf};
use schematic::{Config, ConfigEnum, PartialConfig as _};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    partial::ToPartial,
};

/// `RelativePathBuf` value, used to extend the configuration, optionally
/// specifying a merge strategy.
#[derive(Debug, Clone, PartialEq, Config, Serialize, Deserialize)]
#[serde(untagged)]
#[config(serde(untagged))]
pub enum ExtendingRelativePath {
    /// A relative path without a merge strategy, defaults to
    /// [`ExtendingStrategy::Before`].
    #[setting(default)]
    Path(RelativePathBuf),

    /// A relative path with a custom [`ExtendingStrategy`].
    #[setting(nested)]
    WithStrategy(RelativePathWithStrategy),
}

impl ExtendingRelativePath {
    /// Returns `true` if the path is prepended to the current configuration.
    #[must_use]
    pub const fn is_before(&self) -> bool {
        matches!(
            self,
            Self::Path(_)
                | Self::WithStrategy(RelativePathWithStrategy {
                    strategy: ExtendingStrategy::Before,
                    ..
                })
        )
    }
}

impl From<&RelativePath> for ExtendingRelativePath {
    fn from(value: &RelativePath) -> Self {
        Self::Path(value.to_owned())
    }
}

impl From<&str> for ExtendingRelativePath {
    fn from(value: &str) -> Self {
        Self::Path(RelativePath::new(value).to_owned())
    }
}

impl From<ExtendingRelativePath> for RelativePathBuf {
    fn from(value: ExtendingRelativePath) -> Self {
        match value {
            ExtendingRelativePath::Path(v) => v,
            ExtendingRelativePath::WithStrategy(v) => v.path,
        }
    }
}

impl FromStr for ExtendingRelativePath {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::Path(RelativePath::new(s).to_owned()))
    }
}

impl AsRef<RelativePath> for ExtendingRelativePath {
    fn as_ref(&self) -> &RelativePath {
        match self {
            Self::Path(v) => v.as_relative_path(),
            Self::WithStrategy(v) => v.path.as_ref(),
        }
    }
}

impl Deref for ExtendingRelativePath {
    type Target = RelativePath;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl AssignKeyValue for PartialExtendingRelativePath {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ => match self {
                Self::Path(_) => return missing_key(&kv),
                Self::WithStrategy(config) => config.assign(kv)?,
            },
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialExtendingRelativePath {
    fn delta(&self, next: Self) -> Self {
        if self == &next {
            return Self::empty();
        }

        next
    }
}

impl ToPartial for ExtendingRelativePath {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::Path(v) => Self::Partial::Path(v.clone()),
            Self::WithStrategy(v) => Self::Partial::WithStrategy(v.to_partial()),
        }
    }
}

/// Relative path that is used to extend a configuration using the specified
/// [`ExtendingStrategy`].
#[derive(Debug, Clone, PartialEq, Config, Serialize, Deserialize)]
pub struct RelativePathWithStrategy {
    /// The relative path value.
    pub path: RelativePathBuf,

    /// The load strategy.
    #[setting(default)]
    pub strategy: ExtendingStrategy,
}

impl AssignKeyValue for PartialRelativePathWithStrategy {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "path" => self.path = kv.try_some_string()?.map(RelativePathBuf::from),
            "strategy" => self.strategy = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
impl ToPartial for RelativePathWithStrategy {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            path: Some(self.path.clone()),
            strategy: Some(self.strategy),
        }
    }
}

/// Merge strategy for [`ExtendingRelativePath`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum ExtendingStrategy {
    /// Load the configuration at the path before the current configuration.
    #[default]
    Before,

    /// Load the configuration at the path after the current configuration.
    After,
}
