pub mod code;
pub mod reasoning;
pub mod typewriter;

use confique::Config as Confique;

use crate::error::Result;

/// Style configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique)]
pub struct Config {
    /// Fenced code block style.
    #[config(nested)]
    pub code: code::Config,

    /// Reasoning content style.
    #[config(nested)]
    pub reasoning: reasoning::Config,

    // Typewriter style.
    #[config(nested)]
    pub typewriter: typewriter::Config,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("code.") => self.code.set(path, &key[5..], value)?,
            _ if key.starts_with("reasoning.") => self.reasoning.set(path, &key[10..], value)?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
