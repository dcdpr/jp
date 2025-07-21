pub mod code;
pub mod reasoning;
pub mod tool_call;
pub mod typewriter;

use std::str::FromStr;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
    serde::is_nested_empty,
    Error,
};

/// Style configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Style {
    /// Fenced code block style.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_nested_empty")))]
    pub code: code::Code,

    /// Reasoning content style.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_nested_empty")))]
    pub reasoning: reasoning::Reasoning,

    /// Tool call content style.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_nested_empty")))]
    pub tool_call: tool_call::ToolCall,

    // Typewriter style.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_nested_empty")))]
    pub typewriter: typewriter::Typewriter,
}

impl AssignKeyValue for <Style as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "code" => self.code = kv.try_into_object()?,
            "reasoning" => self.reasoning = kv.try_into_object()?,
            "typewriter" => self.typewriter = kv.try_into_object()?,

            _ if kv.trim_prefix("code") => self.code.assign(kv)?,
            _ if kv.trim_prefix("reasoning") => self.reasoning.assign(kv)?,
            _ if kv.trim_prefix("tool_call") => self.reasoning.assign(kv)?,
            _ if kv.trim_prefix("typewriter") => self.typewriter.assign(kv)?,

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkStyle {
    Off,
    Full,
    Osc8,
}

impl FromStr for LinkStyle {
    type Err = Error;

    fn from_str(style: &str) -> Result<Self> {
        match style {
            "off" => Ok(Self::Off),
            "full" => Ok(Self::Full),
            "osc8" => Ok(Self::Osc8),
            _ => Err(Error::InvalidConfigValueType {
                key: style.to_string(),
                value: style.to_string(),
                need: vec!["off".to_owned(), "full".to_owned(), "osc8".to_owned()],
            }),
        }
    }
}
