pub mod generate;

use confique::Config as Confique;

use crate::error::Result;

/// LLM configuration.
#[derive(Debug, Clone, Default, Confique)]
pub struct Config {
    /// Title generation configuration.
    #[config(nested)]
    pub generate: generate::Config,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("generate.") => self.generate.set(&key[9..], value)?,
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}
