//! Instruction-specific configuration for Jean-Pierre.
//!
//! Instructions are used to guide the assistant in generating a response. They
//! are defined as a list of items, with a title and a list of examples.

use std::str::FromStr;

use schematic::Config;
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, KvAssignment, missing_key},
    assistant::sections::SectionConfig,
    partial::{ToPartial, partial_opt, partial_opts},
};

/// A list of instructions for a persona.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(default, rename_all = "snake_case")]
pub struct InstructionsConfig {
    /// The title of the instructions.
    ///
    /// This is used to organize instructions into sections.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// An optional description of the instructions.
    ///
    /// This is used to provide more context about the instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The position of these instructions.
    ///
    /// A lower position will be shown first. This is useful when merging
    /// multiple instructions, and you want to make sure the most important
    /// instructions are shown first.
    ///
    /// Defaults to `0`.
    #[setting(default = 0)]
    pub position: isize,

    /// The list of instructions.
    ///
    /// Each item is a separate instruction.
    pub items: Vec<String>,

    /// A list of examples to go with the instructions.
    ///
    /// Examples are used to demonstrate how to follow the instructions.
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
            position: partial_opt(&self.position, defaults.position),
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

    /// Convert this instruction into a [`SectionConfig`].
    ///
    /// The instruction's structured data (description, items, examples)
    /// is flattened into markdown content, and the result is wrapped in
    /// an `<instruction>` tag via the section's `tag` field.
    #[must_use]
    #[expect(clippy::cast_possible_truncation)]
    pub fn to_section(&self) -> SectionConfig {
        use std::fmt::Write as _;

        let mut content = String::new();

        if let Some(desc) = &self.description {
            let _ = writeln!(content, "{desc}");
            content.push('\n');
        }

        for item in &self.items {
            let _ = writeln!(content, "- {item}");
        }

        if !self.examples.is_empty() {
            let _ = write!(content, "\n**Examples**\n");
            for example in &self.examples {
                content.push('\n');
                match example {
                    ExampleConfig::Generic(text) => {
                        let _ = writeln!(content, "{text}");
                    }
                    ExampleConfig::Contrast(c) => {
                        let _ = writeln!(content, "Good: {}", c.good);
                        let _ = writeln!(content, "Bad: {}", c.bad);
                        if let Some(reason) = &c.reason {
                            let _ = writeln!(content, "Reason: {reason}");
                        }
                    }
                }
            }
        }

        SectionConfig {
            content: content.trim_end().to_owned(),
            tag: Some("instruction".to_owned()),
            title: self.title.clone(),
            position: self.position as i32,
        }
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
    ///
    /// This is an example of how to follow the instruction.
    pub good: String,

    /// The bad example.
    ///
    /// This is an example of how NOT to follow the instruction.
    pub bad: String,

    /// Why is the good example better than the bad example?
    ///
    /// This is optional, but recommended to provide more context.
    pub reason: Option<String>,
}

impl AssignKeyValue for PartialContrastConfig {
    fn assign(&mut self, kv: KvAssignment) -> Result<(), BoxedError> {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
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
#[path = "instructions_tests.rs"]
mod tests;
