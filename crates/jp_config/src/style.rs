pub mod code;
pub mod reasoning;
pub mod typewriter;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
    is_empty,
};

/// Style configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Style {
    /// Fenced code block style.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub code: code::Code,

    /// Reasoning content style.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub reasoning: reasoning::Reasoning,

    // Typewriter style.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
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
            _ if kv.trim_prefix("typewriter") => self.typewriter.assign(kv)?,

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
