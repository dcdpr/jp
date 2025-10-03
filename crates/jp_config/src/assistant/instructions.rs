//! Instruction-specific configuration for Jean-Pierre.
//!
//! Instructions are used to guide the assistant in generating a response. They
//! are defined as a list of items, with a title and a list of examples.

use std::str::FromStr;

use schematic::Config;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{missing_key, AssignKeyValue, KvAssignment},
    partial::{partial_opt, partial_opts, ToPartial},
    BoxedError,
};

/// A list of instructions for a persona.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(default, rename_all = "snake_case")]
pub struct InstructionsConfig {
    /// The title of the instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// An optional description of the instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The list of instructions.
    pub items: Vec<String>,

    /// A list of examples to go with the instructions.
    #[setting(nested)]
    pub examples: Vec<ExampleConfig>,
}

impl AssignKeyValue for PartialInstructionsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<(), BoxedError> {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            "title" => self.title = kv.try_some_string()?,
            "description" => self.description = kv.try_some_string()?,
            _ if kv.p("items") => kv.try_some_vec_of_strings(&mut self.items)?,
            _ if kv.p("examples") => kv.try_vec_of_nested(&mut self.examples)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl ToPartial for InstructionsConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            title: partial_opts(self.title.as_ref(), defaults.title),
            description: partial_opts(self.description.as_ref(), defaults.description),
            items: partial_opt(&self.items, defaults.items),
            examples: self.examples.iter().map(ToPartial::to_partial).collect(),
        }
    }
}

impl InstructionsConfig {
    /// Add a title to the instructions.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Add a description to the instructions.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add an item to the instructions.
    #[must_use]
    pub fn with_item(mut self, item: impl Into<String>) -> Self {
        self.items.push(item.into());
        self
    }

    /// Serialize the instructions to the proper XML representation.
    ///
    /// # Errors
    ///
    /// Returns an error if the XML serialization fails.
    pub fn try_to_xml(&self) -> Result<String, quick_xml::SeError> {
        #[derive(Serialize)]
        #[serde(rename = "instruction")]
        pub struct XmlWrapper<'a> {
            /// See [`InstructionsConfig::title`].
            #[serde(skip_serializing_if = "Option::is_none", rename = "@title")]
            pub title: Option<&'a str>,

            /// See [`InstructionsConfig::description`].
            #[serde(skip_serializing_if = "Option::is_none")]
            pub description: Option<&'a str>,

            /// See [`InstructionsConfig::items`].
            #[serde(rename = "$value")]
            pub items: Items<'a>,

            /// See [`InstructionsConfig::examples`].
            pub examples: Examples<'a>,
        }

        #[derive(Serialize)]
        struct Items<'a> {
            /// See [`InstructionsConfig::items`].
            #[serde(default, rename = "item")]
            items: &'a [String],
        }

        #[derive(Serialize)]
        struct Examples<'a> {
            /// See [`InstructionsConfig::examples`].
            #[serde(default, rename = "$value")]
            examples: Vec<Example<'a>>,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        enum Example<'a> {
            /// See [`ExampleConfig::Generic`].
            Simple(&'a str),
            /// See [`ExampleConfig::Contrast`].
            Detailed(&'a ContrastConfig),
        }

        let Self {
            title,
            description,
            items,
            examples,
        } = self;

        let wrapper = XmlWrapper {
            title: title.as_deref(),
            description: description.as_deref(),
            items: Items { items },
            examples: Examples {
                examples: examples
                    .iter()
                    .map(|e| match e {
                        ExampleConfig::Generic(text) => Example::Simple(text),
                        ExampleConfig::Contrast(contrast) => Example::Detailed(contrast),
                    })
                    .collect::<Vec<_>>(),
            },
        };

        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);
        serializer.text_format(quick_xml::se::TextFormat::CData);

        wrapper.serialize(serializer)?;
        Ok(buffer)
    }
}

impl FromStr for PartialInstructionsConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            title: Some(s.to_owned()),
            ..Default::default()
        })
    }
}

/// An example part of an instruction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case", serde(untagged))]
pub enum ExampleConfig {
    /// A generic string-based example.
    Generic(String),

    /// A contrast-based (good/bad) example.
    #[setting(nested)]
    Contrast(ContrastConfig),
}

impl AssignKeyValue for PartialExampleConfig {
    fn assign(&mut self, kv: KvAssignment) -> Result<(), BoxedError> {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::Contrast(contrast) => contrast.assign(kv)?,
                Self::Generic(_) => return missing_key(&kv),
            },
        }

        Ok(())
    }
}

impl ToPartial for ExampleConfig {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::Generic(v) => Self::Partial::Generic(v.to_owned()),
            Self::Contrast(v) => Self::Partial::Contrast(v.to_partial()),
        }
    }
}

impl FromStr for PartialExampleConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::Generic(s.to_owned()))
    }
}

/// A contrast-based (good/bad) example.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct ContrastConfig {
    /// The good example.
    pub good: String,

    /// The bad example.
    pub bad: String,

    /// Why is the good example better than the bad example?
    pub reason: Option<String>,
}

impl AssignKeyValue for PartialContrastConfig {
    fn assign(&mut self, kv: KvAssignment) -> Result<(), BoxedError> {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "good" => self.good = kv.try_some_string()?,
            "bad" => self.bad = kv.try_some_string()?,
            "reason" => self.reason = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl ToPartial for ContrastConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            good: partial_opt(&self.good, defaults.good),
            bad: partial_opt(&self.bad, defaults.bad),
            reason: partial_opts(self.reason.as_ref(), defaults.reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instructions_assign() {
        let mut p = PartialInstructionsConfig::default();

        let kv = KvAssignment::try_from_cli("title", "foo").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.title, Some("foo".into()));

        let kv = KvAssignment::try_from_cli("description", "bar").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.description, Some("bar".into()));

        let kv = KvAssignment::try_from_cli("items", "baz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.items, Some(vec!["baz".into()]));

        let kv = KvAssignment::try_from_cli("items+", "quux").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.items, Some(vec!["baz".into(), "quux".into()]));

        let kv = KvAssignment::try_from_cli("items.0", "quuz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.items, Some(vec!["quuz".into(), "quux".into()]));

        let kv = KvAssignment::try_from_cli("examples", "qux").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.examples, vec![PartialExampleConfig::Generic(
            "qux".into()
        )]);

        let kv = KvAssignment::try_from_cli("examples+", "quuz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.examples, vec![
            PartialExampleConfig::Generic("qux".into()),
            PartialExampleConfig::Generic("quuz".into())
        ]);

        let kv = KvAssignment::try_from_cli("examples.0", "quuz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.examples, vec![
            PartialExampleConfig::Generic("quuz".into()),
            PartialExampleConfig::Generic("quuz".into())
        ]);
    }

    #[test]
    fn test_example_assign() {
        let mut p = PartialExampleConfig::default();

        let kv = KvAssignment::try_from_cli("", "bar").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p, PartialExampleConfig::Generic("bar".into()));

        let kv = KvAssignment::try_from_cli(":", r#""bar""#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p, PartialExampleConfig::Generic("bar".into()));

        let kv = KvAssignment::try_from_cli(":", r#"{"good":"bar","bad":"baz"}"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p,
            PartialExampleConfig::Contrast(PartialContrastConfig {
                good: Some("bar".into()),
                bad: Some("baz".into()),
                reason: None,
            })
        );

        let kv = KvAssignment::try_from_cli("nope", "nope").unwrap();
        assert_eq!(&p.assign(kv).unwrap_err().to_string(), "nope: unknown key");
    }

    #[test]
    fn test_contrast_assign() {
        let mut p = PartialContrastConfig::default();

        let kv = KvAssignment::try_from_cli("good", "bar").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p, PartialContrastConfig {
            good: Some("bar".into()),
            bad: None,
            reason: None,
        });

        let kv = KvAssignment::try_from_cli("bad", "baz").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p, PartialContrastConfig {
            good: Some("bar".into()),
            bad: Some("baz".into()),
            reason: None,
        });

        let kv = KvAssignment::try_from_cli("reason", "qux").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p, PartialContrastConfig {
            good: Some("bar".into()),
            bad: Some("baz".into()),
            reason: Some("qux".into()),
        });

        let kv = KvAssignment::try_from_cli(":", r#"{"good":"one","bad":null}"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p, PartialContrastConfig {
            good: Some("one".into()),
            bad: None,
            reason: None,
        });

        let kv = KvAssignment::try_from_cli("nope", "nope").unwrap();
        assert_eq!(&p.assign(kv).unwrap_err().to_string(), "nope: unknown key");
    }

    #[test]
    fn test_instructions_to_xml() {
        let i = InstructionsConfig {
            title: Some("foo".to_owned()),
            description: Some("bar".to_owned()),
            items: vec![
                "foo".to_owned(),
                "bar <test>bar</test>".to_owned(),
                "baz]]> baz".to_owned(),
            ],
            examples: vec![
                ExampleConfig::Generic("foo".to_owned()),
                ExampleConfig::Contrast(ContrastConfig {
                    good: "bar".to_owned(),
                    bad: "baz".to_owned(),
                    reason: Some("qux".to_owned()),
                }),
                ExampleConfig::Contrast(ContrastConfig {
                    good: "quux".to_owned(),
                    bad: "quuz".to_owned(),
                    reason: None,
                }),
            ],
        };

        let xml = i.try_to_xml().unwrap();
        insta::assert_snapshot!(xml);
    }
}
