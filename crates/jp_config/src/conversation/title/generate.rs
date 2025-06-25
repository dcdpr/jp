use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
    is_default, is_empty, model,
};

/// LLM configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Generate {
    /// Model configuration for title generation.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub model: model::Model,

    /// Whether to generate a title automatically for new conversations.
    #[config(
        default = true,
        partial_attr(serde(skip_serializing_if = "is_default"))
    )]
    pub auto: bool,
}

impl AssignKeyValue for <Generate as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "model" => self.model = kv.try_into_object()?,
            "auto" => self.auto = Some(kv.try_into_bool()?),

            _ if kv.trim_prefix("model") => self.model.assign(kv)?,

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
