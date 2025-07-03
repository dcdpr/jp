pub mod provider;

use confique::Config as Confique;
use jp_mcp::tool::ToolChoice;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
    model,
    serde::{de_from_str_opt, is_default, is_nested_empty},
};

pub type AssistantPartial = <Assistant as Confique>::Partial;

/// LLM configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Assistant {
    /// Optional name of the assistant.
    #[config(partial_attr(serde(skip_serializing_if = "is_default")))]
    pub name: Option<String>,

    /// The system prompt to use for the assistant.
    #[config(default = "You are a helpful assistant.")]
    pub system_prompt: String,

    /// A list of instructions for the assistant.
    #[config(
        default = [],
        partial_attr(serde(skip_serializing_if = "Option::is_none"))
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<Instructions>,

    /// How the LLM should choose tools, if any are available.
    #[config(partial_attr(serde(default, deserialize_with = "de_from_str_opt")))]
    pub tool_choice: Option<ToolChoice>,

    /// Provider configuration.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_nested_empty")))]
    pub provider: provider::Provider,

    /// Model configuration.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_nested_empty")))]
    pub model: model::Model,
}

impl AssignKeyValue for <Assistant as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "provider" => self.provider = kv.try_into_object()?,
            "model" => self.model = kv.try_into_object()?,
            "name" => self.name = Some(kv.try_into_string()?),
            "system_prompt" => self.system_prompt = Some(kv.try_into_string()?),
            "instructions" => {
                kv.try_set_or_merge_vec(self.instructions.get_or_insert_default(), |v| match v {
                    Value::String(v) => Ok(Instructions::new(v)),
                    v @ Value::Object(_) => Ok(serde_json::from_value(v)?),
                    _ => Err("Expected string or object".into()),
                })?;
            }
            "tool_choice" => self.tool_choice = Some(kv.try_into_string()?.parse()?),

            _ if kv.trim_prefix("provider") => self.provider.assign(kv)?,
            _ if kv.trim_prefix("model") => self.model.assign(kv)?,

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}

/// A list of instructions for a persona.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Instructions {
    /// The title of the instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// An optional description of the instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The list of instructions.
    #[serde(default)]
    pub items: Vec<String>,

    /// A list of examples to go with the instructions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

impl Instructions {
    pub fn try_to_xml(&self) -> Result<String> {
        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);
        self.serialize(serializer)?;
        Ok(buffer)
    }

    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            description: None,
            items: vec![],
            examples: vec![],
        }
    }

    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn with_item(mut self, item: impl Into<String>) -> Self {
        self.items.push(item.into());
        self
    }
}
