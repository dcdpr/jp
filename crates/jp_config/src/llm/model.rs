use std::str::FromStr;

use confique::Config as Confique;
use jp_conversation::{model::Parameters, ModelId};
use serde::Deserialize;

use crate::error::Result;

/// Model configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique)]
pub struct Config {
    /// Model to use, regardless of the conversation context.
    ///
    /// If not set (default), the model will be determined by the conversation
    /// context.
    #[config(env = "JP_LLM_MODEL_ID", deserialize_with = de_model_id)]
    pub id: Option<ModelId>,

    /// The parameters to use for the model.
    #[config(env = "JP_LLM_MODEL_PARAMETERS")]
    pub parameters: Option<Parameters>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        let value: String = value.into();

        match key {
            _ if key.starts_with("parameters.") => {
                self.parameters
                    .get_or_insert_default()
                    .set(&key[11..], value)?;
            }
            "id" => self.id = (!value.is_empty()).then(|| value.parse()).transpose()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

pub fn de_model_id<'de, D>(deserializer: D) -> std::result::Result<ModelId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string: String = String::deserialize(deserializer)?;
    ModelId::from_str(&string).map_err(serde::de::Error::custom)
}
