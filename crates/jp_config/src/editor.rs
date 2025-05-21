use confique::Config as Confique;

use crate::{error::Result, parse_vec};

/// LLM configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique)]
pub struct Config {
    /// The command to use for editing text.
    ///
    /// If unset, falls back to `env_vars`.
    #[config(env = "JP_EDITOR_CMD")]
    pub cmd: Option<String>,

    /// The environment variables to use for editing text. Used if `cmd` is
    /// unset.
    ///
    /// Defaults to `JP_EDITOR`, `VISUAL`, and `EDITOR`.
    #[config(default = ["JP_EDITOR", "VISUAL", "EDITOR"], env = "JP_EDITOR_ENV_VARS")]
    pub env_vars: Vec<String>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        let value: String = value.into();

        match key {
            "cmd" => self.cmd = (!value.is_empty()).then_some(value),
            "env_vars" => self.env_vars = parse_vec(&value, str::to_owned),
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
