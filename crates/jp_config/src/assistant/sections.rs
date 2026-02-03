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

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_instructions_assign() {
//         let mut p = PartialSectionConfig::default();
//
//         let kv = KvAssignment::try_from_cli("title", "foo").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p.title, Some("foo".into()));
//
//         let kv = KvAssignment::try_from_cli("description", "bar").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p.description, Some("bar".into()));
//
//         let kv = KvAssignment::try_from_cli("items", "baz").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p.items, Some(vec!["baz".into()]));
//
//         let kv = KvAssignment::try_from_cli("items+", "quux").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p.items, Some(vec!["baz".into(), "quux".into()]));
//
//         let kv = KvAssignment::try_from_cli("items.0", "quuz").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p.items, Some(vec!["quuz".into(), "quux".into()]));
//
//         let kv = KvAssignment::try_from_cli("examples", "qux").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p.examples, vec![PartialExampleConfig::Generic(
//             "qux".into()
//         )]);
//
//         let kv = KvAssignment::try_from_cli("examples+", "quuz").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p.examples, vec![
//             PartialExampleConfig::Generic("qux".into()),
//             PartialExampleConfig::Generic("quuz".into())
//         ]);
//
//         let kv = KvAssignment::try_from_cli("examples.0", "quuz").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p.examples, vec![
//             PartialExampleConfig::Generic("quuz".into()),
//             PartialExampleConfig::Generic("quuz".into())
//         ]);
//     }
//
//     #[test]
//     fn test_example_assign() {
//         let mut p = PartialExampleConfig::default();
//
//         let kv = KvAssignment::try_from_cli("", "bar").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p, PartialExampleConfig::Generic("bar".into()));
//
//         let kv = KvAssignment::try_from_cli(":", r#""bar""#).unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p, PartialExampleConfig::Generic("bar".into()));
//
//         let kv = KvAssignment::try_from_cli(":", r#"{"good":"bar","bad":"baz"}"#).unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(
//             p,
//             PartialExampleConfig::Contrast(PartialContrastConfig {
//                 good: Some("bar".into()),
//                 bad: Some("baz".into()),
//                 reason: None,
//             })
//         );
//
//         let kv = KvAssignment::try_from_cli("nope", "nope").unwrap();
//         assert_eq!(&p.assign(kv).unwrap_err().to_string(), "nope: unknown key");
//     }
//
//     #[test]
//     fn test_contrast_assign() {
//         let mut p = PartialContrastConfig::default();
//
//         let kv = KvAssignment::try_from_cli("good", "bar").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p, PartialContrastConfig {
//             good: Some("bar".into()),
//             bad: None,
//             reason: None,
//         });
//
//         let kv = KvAssignment::try_from_cli("bad", "baz").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p, PartialContrastConfig {
//             good: Some("bar".into()),
//             bad: Some("baz".into()),
//             reason: None,
//         });
//
//         let kv = KvAssignment::try_from_cli("reason", "qux").unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p, PartialContrastConfig {
//             good: Some("bar".into()),
//             bad: Some("baz".into()),
//             reason: Some("qux".into()),
//         });
//
//         let kv = KvAssignment::try_from_cli(":", r#"{"good":"one","bad":null}"#).unwrap();
//         p.assign(kv).unwrap();
//         assert_eq!(p, PartialContrastConfig {
//             good: Some("one".into()),
//             bad: None,
//             reason: None,
//         });
//
//         let kv = KvAssignment::try_from_cli("nope", "nope").unwrap();
//         assert_eq!(&p.assign(kv).unwrap_err().to_string(), "nope: unknown key");
//     }
//
//     #[test]
//     fn test_instructions_to_xml() {
//         let i = SectionConfig {
//             title: Some("foo".to_owned()),
//             description: Some("bar".to_owned()),
//             position: 0,
//             items: vec![
//                 "foo".to_owned(),
//                 "bar <test>bar</test>".to_owned(),
//                 "baz]]> baz".to_owned(),
//             ],
//             examples: vec![
//                 ExampleConfig::Generic("foo".to_owned()),
//                 ExampleConfig::Contrast(ContrastConfig {
//                     good: "bar".to_owned(),
//                     bad: "baz".to_owned(),
//                     reason: Some("qux".to_owned()),
//                 }),
//                 ExampleConfig::Contrast(ContrastConfig {
//                     good: "quux".to_owned(),
//                     bad: "quuz".to_owned(),
//                     reason: None,
//                 }),
//             ],
//         };
//
//         let xml = i.try_to_xml().unwrap();
//         insta::assert_snapshot!(xml);
//     }
// }
