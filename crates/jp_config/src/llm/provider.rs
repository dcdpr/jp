pub mod anthropic;
pub mod deepseek;
pub mod google;
pub mod openai;
pub mod openrouter;

use confique::Config as Confique;

use crate::error::Result;

/// Provider configuration.
#[derive(Debug, Clone, Confique)]
pub struct Config {
    /// Anthropic API configuration.
    #[config(nested)]
    pub anthropic: anthropic::Config,

    /// Deepseek API configuration.
    #[config(nested)]
    pub deepseek: deepseek::Config,

    /// Google API configuration.
    #[config(nested)]
    pub google: google::Config,

    /// Openrouter API configuration.
    #[config(nested)]
    pub openrouter: openrouter::Config,

    /// Openai API configuration.
    #[config(nested)]
    pub openai: openai::Config,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("anthropic.") => self.anthropic.set(&key[10..], value)?,
            _ if key.starts_with("deepseek.") => self.deepseek.set(&key[9..], value)?,
            _ if key.starts_with("google.") => self.google.set(&key[7..], value)?,
            _ if key.starts_with("openrouter.") => self.openrouter.set(&key[11..], value)?,
            _ if key.starts_with("openai.") => self.openai.set(&key[7..], value)?,
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}
