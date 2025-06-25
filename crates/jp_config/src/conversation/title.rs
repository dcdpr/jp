pub mod generate;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// LLM configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Title {
    /// Title generation configuration.
    #[config(nested)]
    pub generate: generate::Generate,
}

impl AssignKeyValue for <Title as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "generate" => self.generate = kv.try_into_object()?,

            _ if kv.trim_prefix("generate") => self.generate.assign(kv)?,

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
