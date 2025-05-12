pub mod title;

use confique::Config as Confique;

use crate::error::Result;

/// LLM configuration.
#[derive(Debug, Clone, Default, Confique)]
pub struct Config {
    /// Title configuration.
    #[config(nested)]
    pub title: title::Config,

    /// Persona to use for the active conversation.
    ///
    /// If unset, uses the `default` persona, if one exists.
    #[config(env = "JP_CONVERSATION_PERSONA")]
    pub persona: Option<String>,

    /// Context to use for the active conversation.
    ///
    /// If unset, uses the `default` context, if one exists.
    #[config(env = "JP_CONVERSATION_CONTEXT")]
    pub context: Option<String>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("title.") => self.title.set(&key[6..], value)?,
            "persona" => self.persona = Some(value.into()),
            "context" => self.context = Some(value.into()),
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}
