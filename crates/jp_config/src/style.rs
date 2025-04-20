pub mod code;

use confique::Config as Confique;

use crate::error::Result;

/// Style configuration.
#[derive(Debug, Clone, Confique)]
pub struct Config {
    /// Fenced code block style.
    #[config(nested)]
    pub code: code::Config,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("code.") => self.code.set(&key[5..], value)?,
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}
