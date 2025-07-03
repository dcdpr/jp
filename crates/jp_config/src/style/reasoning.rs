use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Reasoning style configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Reasoning {
    /// Whether to show the "reasoning" text.
    ///
    /// Even if this is disabled, the model will still generate reasoning text,
    /// but it will not be displayed.
    #[config(default = true)]
    pub show: bool,
}

impl AssignKeyValue for <Reasoning as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "show" => self.show = Some(kv.try_into_bool()?),

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}
