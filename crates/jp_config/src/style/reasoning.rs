use confique::Config as Confique;

use crate::error::Result;

/// Reasoning style configuration.
#[derive(Debug, Clone, PartialEq, Confique)]
pub struct Config {
    /// Whether to show the "reasoning" text.
    ///
    /// Even if this is disabled, the model will still generate reasoning text,
    /// but it will not be displayed.
    #[config(default = true, env = "JP_STYLE_REASONING_SHOW")]
    pub show: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { show: true }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "show" => self.show = value.into().parse()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
