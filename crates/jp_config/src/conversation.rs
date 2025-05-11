pub mod title;

use confique::Config as Confique;

use crate::error::Result;

/// LLM configuration.
#[derive(Debug, Clone, Default, Confique)]
pub struct Config {
    /// Title configuration.
    #[config(nested)]
    pub title: title::Config,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("title.") => self.title.set(&key[6..], value)?,
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}
