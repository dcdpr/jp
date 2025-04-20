pub mod provider;

use std::str::FromStr;

use confique::Config as Confique;
use jp_conversation::model::ProviderId;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// LLM configuration.
#[derive(Debug, Clone, Confique)]
pub struct Config {
    /// Provider configuration.
    #[config(nested)]
    pub provider: provider::Config,

    /// Model to use, regardless of the conversation context.
    ///
    /// If not set (default), the model will be determined by the conversation
    /// context.
    #[config(env = "JP_LLM_MODEL", deserialize_with = deserialize_model)]
    pub model: Option<ProviderModelSlug>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("provider.") => self.provider.set(&key[9..], value)?,
            "model" => self.model = Some(value.into().parse()?),
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderModelSlug {
    pub provider: ProviderId,
    pub slug: String,
}

impl FromStr for ProviderModelSlug {
    type Err = Error;

    fn from_str(slug: &str) -> Result<Self> {
        let (provider, model) = slug.split_once('/').ok_or(Error::ModelSlug(
            "format must be 'provider/model'".to_owned(),
        ))?;

        if provider.is_empty() || model.is_empty() {
            return Err(Error::ModelSlug(
                "format must be 'provider/model'".to_string(),
            ));
        }

        match provider {
            "openrouter" | "openai" | "google" | "deepseek" | "anthropic" => (),
            _ => return Err(Error::ModelSlug(format!("unknown provider: {provider}"))),
        }

        Ok(Self {
            provider: ProviderId::from_str(provider)?,
            slug: model.to_string(),
        })
    }
}

pub fn deserialize_model<'de, D>(
    deserializer: D,
) -> std::result::Result<ProviderModelSlug, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string: String = String::deserialize(deserializer)?;
    ProviderModelSlug::from_str(&string).map_err(serde::de::Error::custom)
}
