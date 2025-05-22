use std::{collections::HashMap, str::FromStr};

use confique::Config as Confique;
use jp_conversation::{model::ProviderId, Model, ModelId};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Model configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique)]
pub struct Config {
    /// Model to use, regardless of the conversation context.
    ///
    /// If not set (default), the model will be determined by the conversation
    /// context.
    #[config(env = "JP_LLM_MODEL_SLUG", deserialize_with = de_slug)]
    pub slug: Option<ProviderModelSlug>,

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
            "slug" => self.slug = (!value.is_empty()).then(|| value.parse()).transpose()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderModelSlug {
    pub provider: ProviderId,
    pub slug: String,
}

impl From<ProviderModelSlug> for Model {
    fn from(slug: ProviderModelSlug) -> Self {
        Self::new(slug.provider, slug.slug)
    }
}

impl TryFrom<ProviderModelSlug> for ModelId {
    type Error = Error;

    fn try_from(slug: ProviderModelSlug) -> Result<Self> {
        Self::try_from((slug.provider, slug.slug)).map_err(Into::into)
    }
}

impl FromStr for ProviderModelSlug {
    type Err = Error;

    fn from_str(slug: &str) -> Result<Self> {
        let (provider, model) = slug.split_once('/').ok_or(Error::ModelSlug(
            "format must be '<provider>/<model>'".to_owned(),
        ))?;

        if model.is_empty() {
            return Err(Error::ModelSlug(
                "format must be '<provider>/<model>'".to_string(),
            ));
        }

        Ok(Self {
            provider: ProviderId::from_str(provider)?,
            slug: model.to_string(),
        })
    }
}

pub fn de_slug<'de, D>(deserializer: D) -> std::result::Result<ProviderModelSlug, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string: String = String::deserialize(deserializer)?;
    ProviderModelSlug::from_str(&string).map_err(serde::de::Error::custom)
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
