//! System prompt sections.

use std::str::FromStr;

use schematic::Config;
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt, partial_opts},
};

/// Command configuration, either as a string or a complete configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case", serde(untagged))]
#[serde(untagged)]
pub enum SectionConfigOrString {
    /// A single string, which is interpreted as the full content of the
    /// section.
    String(String),

    /// A complete section configuration.
    #[setting(nested)]
    Config(SectionConfig),
}

impl AssignKeyValue for PartialSectionConfigOrString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::String(_) => return missing_key(&kv),
                Self::Config(config) => config.assign(kv)?,
            },
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialSectionConfigOrString {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Config(prev), Self::Config(next)) => Self::Config(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for SectionConfigOrString {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::String(v) => Self::Partial::String(v.to_owned()),
            Self::Config(v) => Self::Partial::Config(v.to_partial()),
        }
    }
}

impl FromStr for PartialSectionConfigOrString {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::String(s.to_owned()))
    }
}

/// A list of sections for a system prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(default, rename_all = "snake_case")]
pub struct SectionConfig {
    /// The content of the section.
    pub content: String,

    /// Optional tag surrounding the section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,

    /// The position of the section.
    ///
    /// A lower position will be shown first. This is useful when merging
    /// multiple sections, and you want to make sure the most important
    /// sections are shown first.
    ///
    /// Defaults to `0`.
    #[setting(default = 0)]
    pub position: i32,
}

impl AssignKeyValue for PartialSectionConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            "tag" => self.tag = kv.try_some_string()?,
            "content" => self.content = kv.try_some_string()?,
            "position" => self.position = kv.try_some_i32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl ToPartial for SectionConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            tag: partial_opts(self.tag.as_ref(), defaults.tag),
            content: partial_opt(&self.content, defaults.content),
            position: partial_opt(&self.position, defaults.position),
        }
    }
}

impl PartialConfigDelta for PartialSectionConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            tag: delta_opt(self.tag.as_ref(), next.tag),
            content: delta_opt(self.content.as_ref(), next.content),
            position: delta_opt(self.position.as_ref(), next.position),
        }
    }
}

impl SectionConfig {
    /// Add a tag to the section.
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Add content to the section.
    #[must_use]
    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }
}

impl FromStr for PartialSectionConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            content: Some(s.to_owned()),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instructions_assign() {
        let mut p = PartialSectionConfig::default();

        let kv = KvAssignment::try_from_cli("tag", "foo").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.tag, Some("foo".into()));

        let kv = KvAssignment::try_from_cli("content", "bar").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.content, Some("bar".into()));

        let kv = KvAssignment::try_from_cli("position", "1").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.position, Some(1));
    }
}
