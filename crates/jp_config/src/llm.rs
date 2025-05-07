pub mod provider;

use std::str::FromStr;

use confique::Config as Confique;
use jp_conversation::{model::ProviderId, Model, ModelId};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// LLM configuration.
#[derive(Debug, Clone, Default, Confique)]
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

    /// How the LLM should choose tools, if any are available.
    #[config(default = "auto", env = "JP_LLM_TOOL_CHOICE", deserialize_with = de_tool_choice)]
    pub tool_choice: ToolChoice,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("provider.") => self.provider.set(&key[9..], value)?,
            "model" => self.model = Some(value.into().parse()?),
            "tool_choice" => self.tool_choice = value.into().parse()?,
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

pub fn deserialize_model<'de, D>(
    deserializer: D,
) -> std::result::Result<ProviderModelSlug, D::Error>
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

fn de_tool_choice<'de, D>(deserializer: D) -> std::result::Result<ToolChoice, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string: String = String::deserialize(deserializer)?;
    ToolChoice::from_str(&string).map_err(serde::de::Error::custom)
}
