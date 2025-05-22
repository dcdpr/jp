use std::collections::HashMap;

use confique::Config as Confique;

use crate::{
    error::Result,
    llm::{model::de_slug, ProviderModelSlug},
};

/// Model configuration.
#[derive(Debug, Clone, PartialEq, Confique)]
pub struct Config {
    /// Model to use for title generation.
    #[config(default = "openai/gpt-4.1-nano", env = "JP_CONVERSATION_TITLE_GENERATE_MODEL_SLUG", deserialize_with = de_slug)]
    pub slug: ProviderModelSlug,

    /// The parameters to use for the model.
    #[config(default = {}, env = "JP_CONVERSATION_TITLE_GENERATE_MODEL_PARAMETERS")]
    pub parameters: HashMap<String, serde_json::Value>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        let value: String = value.into();

        match key {
            _ if key.starts_with("parameters.") => {
                self.parameters
                    .insert(key[11..].to_owned(), serde_json::from_str(&value)?);
            }
            "slug" => self.slug = value.parse()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            slug: "openai/gpt-4.1-nano".parse().unwrap(),
            parameters: HashMap::new(),
        }
    }
}
