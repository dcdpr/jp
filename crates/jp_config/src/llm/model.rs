use std::{collections::HashMap, str::FromStr};

use confique::Config as Confique;
use jp_conversation::ModelId;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Model configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique)]
pub struct Config {
    /// Model to use, regardless of the conversation context.
    ///
    /// If not set (default), the model will be determined by the conversation
    /// context.
    #[config(env = "JP_LLM_MODEL_ID", deserialize_with = de_model_id)]
    pub id: Option<ModelId>,

    /// The parameters to use for the model.
    #[config(default = {}, env = "JP_LLM_MODEL_PARAMETERS")]
    pub parameters: HashMap<String, serde_json::Value>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        let value: String = value.into();

        match key {
            _ if key.starts_with("parameters.") => {
                self.parameters
                    .insert(key[11..].to_owned(), serde_json::from_str(&value)?);
            }
            "id" => self.id = (!value.is_empty()).then(|| value.parse()).transpose()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

pub fn de_model_id<'de, D>(deserializer: D) -> std::result::Result<ModelId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string: String = String::deserialize(deserializer)?;
    ModelId::from_str(&string).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Function(String),
}

impl FromStr for ToolChoice {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "auto" => Ok(Self::Auto),
            "none" => Ok(Self::None),
            "required" => Ok(Self::Required),
            _ if s.starts_with("fn:") && s.len() > 3 => Ok(Self::Function(s[3..].to_owned())),
            _ => Err(Error::InvalidConfigValue {
                key: s.to_string(),
                value: s.to_string(),
                need: vec![
                    "auto".to_owned(),
                    "none".to_owned(),
                    "required".to_owned(),
                    "fn:<name>".to_owned(),
                ],
            }),
        }
    }
}
