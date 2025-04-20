use confique::{meta::FieldKind, Config as Confique};

use crate::{error::Result, llm, style};

/// Workspace Configuration.
#[derive(Debug, Clone, Confique)]
pub struct Config {
    /// Inherit from a local ancestor or global configuration file.
    #[config(default = true)]
    pub inherit: bool,

    /// LLM-specific configuration.
    #[config(nested)]
    pub llm: llm::Config,

    /// Styling configuration.
    #[config(nested)]
    pub style: style::Config,
}

impl Config {
    #[must_use]
    pub fn fields() -> Vec<String> {
        let mut output = Vec::new();
        let mut stack = vec![(&Self::META, String::new())];

        while let Some((meta, prefix)) = stack.pop() {
            for field in meta.fields {
                let mut path = field.name.to_string();
                if !prefix.is_empty() {
                    path = format!("{prefix}.{path}");
                }

                if let FieldKind::Nested { meta } = field.kind {
                    stack.push((meta, path));
                } else {
                    output.push(path);
                }
            }
        }

        output
    }

    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "inherit" => self.inherit = value.into().parse()?,
            _ if key.starts_with("llm.") => self.llm.set(&key[4..], value)?,
            _ if key.starts_with("style.") => self.style.set(&key[6..], value)?,
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}
