mod model;

use confique::Config as Confique;

use crate::error::Result;

/// LLM configuration.
#[derive(Debug, Clone, PartialEq, Confique)]
pub struct Config {
    /// Model configuration for title generation.
    // TODO: Figure out a way to re-use `jp_config::llm::model::Config` here,
    // but the environment variables and defaults differ.
    #[config(nested)]
    pub model: model::Config,

    /// Whether to generate a title automatically for new conversations.
    #[config(default = true, env = "JP_CONVERSATION_TITLE_GENERATE_AUTO")]
    pub auto: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: model::Config::default(),
            auto: true,
        }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("model.") => self.model.set(path, &key[6..], value)?,
            "auto" => self.auto = value.into().parse()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
