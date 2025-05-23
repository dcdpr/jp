use confique::Config as Confique;
use jp_conversation::{model::Parameters, ModelId};

use crate::{error::Result, llm::model::de_model_id};

/// Model configuration.
#[derive(Debug, Clone, PartialEq, Confique)]
pub struct Config {
    /// Model to use for title generation.
    #[config(default = "openai/gpt-4.1-nano", env = "JP_CONVERSATION_TITLE_GENERATE_MODEL_ID", deserialize_with = de_model_id)]
    pub id: ModelId,

    /// The parameters to use for the model.
    #[config(env = "JP_CONVERSATION_TITLE_GENERATE_MODEL_PARAMETERS")]
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
            "id" => self.id = value.parse()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            id: "openai/gpt-4.1-nano".parse().unwrap(),
            parameters: None,
        }
    }
}
