use confique::Config as Confique;

use crate::{
    error::Result,
    llm::{de_model, ProviderModelSlug},
};

/// LLM configuration.
#[derive(Debug, Clone, Confique)]
pub struct Config {
    /// Model to use for title generation.
    #[config(default = "openai/gpt-4.1-nano", env = "JP_CONVERSATION_TITLE_GENERATE_MODEL", deserialize_with = de_model)]
    pub model: ProviderModelSlug,

    /// Whether to generate a title automatically for new conversations.
    #[config(default = true, env = "JP_CONVERSATION_TITLE_GENERATE_AUTO")]
    pub auto: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "openai/gpt-4.1-nano".parse().unwrap(),
            auto: true,
        }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "model" => self.model = value.into().parse()?,
            "auto" => self.auto = value.into().parse()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
