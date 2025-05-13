use std::collections::HashMap;

use confique::Config as Confique;
use serde_json::Value;

use crate::error::Result;

/// Template configuration.
#[derive(Debug, Clone, Default, Confique)]
pub struct Config {
    /// Template variable values used to render query templates.
    #[config(default = {})]
    pub values: HashMap<String, Value>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("values.") => {
                let mut parts = key[7..].split('.').peekable();
                let mut template_values = serde_json::Map::new();
                let mut values = &mut template_values;

                while let Some(segment) = parts.next() {
                    if parts.peek().is_none() {
                        values.insert(segment.to_owned(), serde_json::from_str(&value.into())?);
                        break;
                    }

                    let next_val = values
                        .entry(segment.to_owned())
                        .or_insert(serde_json::json!({}));

                    if !next_val.is_object() {
                        *next_val = serde_json::json!({});
                    }

                    values = next_val.as_object_mut().unwrap();
                }

                self.values.extend(template_values);
            }
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
