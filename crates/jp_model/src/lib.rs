mod error;

use std::{fmt, str::FromStr};

use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id,
};
use serde::{Deserialize, Serialize};

pub use crate::error::Error;
use crate::error::Result;

/// The ID of a model, composed of a provider and a slug.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModelId {
    provider: ProviderId,
    slug: String,
}

impl ModelId {
    /// The provider of the model.
    #[must_use]
    pub fn provider(&self) -> ProviderId {
        self.provider
    }

    /// The slug of the model.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }
}

impl Id for ModelId {
    fn variant() -> Variant {
        'm'.into()
    }

    fn target_id(&self) -> TargetId {
        self.to_string().into()
    }

    fn global_id(&self) -> GlobalId {
        jp_id::global::get().into()
    }

    fn is_valid(&self) -> bool {
        Self::variant().is_valid() && self.global_id().is_valid()
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider.target_id(), self.slug)
    }
}

impl From<ModelId> for String {
    fn from(model: ModelId) -> Self {
        model.to_string()
    }
}

impl TryFrom<&str> for ModelId {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self> {
        Self::try_from(s.to_owned())
    }
}

impl TryFrom<&String> for ModelId {
    type Error = Error;

    fn try_from(s: &String) -> Result<Self> {
        Self::try_from(s.as_str())
    }
}

impl TryFrom<String> for ModelId {
    type Error = Error;

    fn try_from(s: String) -> Result<Self> {
        Self::from_str(s.as_str())
    }
}

impl TryFrom<(ProviderId, String)> for ModelId {
    type Error = Error;

    fn try_from((provider, name): (ProviderId, String)) -> Result<Self> {
        Self::try_from((provider, name.as_str()))
    }
}

impl TryFrom<(ProviderId, &String)> for ModelId {
    type Error = Error;

    fn try_from((provider, name): (ProviderId, &String)) -> Result<Self> {
        Self::try_from((provider, name.as_str()))
    }
}

impl TryFrom<(ProviderId, &str)> for ModelId {
    type Error = Error;

    fn try_from((provider, name): (ProviderId, &str)) -> Result<Self> {
        Self::try_from(format!("{provider}/{name}"))
    }
}

impl FromStr for ModelId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let (provider, name) =
            s.split_once('/')
                .map(|(p, n)| (p.trim(), n.trim()))
                .ok_or(Error::InvalidIdFormat(
                    "ID must match <provider>/<model>".to_owned(),
                ))?;

        if name.is_empty()
            || name.chars().any(|c| {
                !(c.is_numeric()
                    || c.is_ascii_alphabetic()
                    || c == '-'
                    || c == '_'
                    || c == '.'
                    || c == ':'
                    || c == '/')
            })
        {
            return Err(Error::InvalidIdFormat(
                "Model ID must be [a-zA-Z0-9_-.:/]+".to_string(),
            ));
        }

        Ok(Self {
            provider: ProviderId::from_str(provider)?,
            slug: name.to_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Anthropic,
    Deepseek,
    Google,
    Llamacpp,
    Ollama,
    Openai,
    #[default]
    Openrouter,
    Xai,
}

impl Id for ProviderId {
    fn variant() -> Variant {
        'p'.into()
    }

    fn target_id(&self) -> TargetId {
        self.to_string().into()
    }

    fn global_id(&self) -> GlobalId {
        jp_id::global::get().into()
    }

    fn is_valid(&self) -> bool {
        Self::variant().is_valid() && self.global_id().is_valid()
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anthropic => f.write_str("anthropic"),
            Self::Deepseek => f.write_str("deepseek"),
            Self::Google => f.write_str("google"),
            Self::Llamacpp => f.write_str("llamacpp"),
            Self::Openai => f.write_str("openai"),
            Self::Openrouter => f.write_str("openrouter"),
            Self::Ollama => f.write_str("ollama"),
            Self::Xai => f.write_str("xai"),
        }
    }
}

impl FromStr for ProviderId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "anthropic" => Ok(Self::Anthropic),
            "deepseek" => Ok(Self::Deepseek),
            "google" => Ok(Self::Google),
            "llamacpp" => Ok(Self::Llamacpp),
            "openai" => Ok(Self::Openai),
            "openrouter" => Ok(Self::Openrouter),
            "ollama" => Ok(Self::Ollama),
            _ if s.is_empty() => Err(Error::InvalidProviderId("<empty>".to_owned())),
            _ => Err(Error::InvalidProviderId(s.to_owned())),
        }
    }
}
