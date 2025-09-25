//! LLM model ID configuration.

use std::{fmt, str::FromStr};

use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id,
};
use schematic::{Config, ConfigEnum, Schematic};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::{delta_opt, PartialConfigDelta},
};

/// Assistant-specific configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct ModelIdConfig {
    /// The provider to supply the model.
    #[setting(required)]
    pub provider: ProviderId,

    /// The actual model name.
    #[setting(required)]
    pub name: Name,
}

impl AssignKeyValue for PartialModelIdConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            "provider" => self.provider = kv.try_some_from_str()?,
            "name" => self.name = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialModelIdConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            provider: delta_opt(self.provider.as_ref(), next.provider),
            name: delta_opt(self.name.as_ref(), next.name),
        }
    }
}

impl fmt::Display for ModelIdConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.name)
    }
}

/// Error when parsing `ModelIdConfig`.
#[derive(Debug, thiserror::Error)]
pub enum ModelIdConfigError {
    /// Error when parsing `ModelIdConfig` from a string.
    #[error("model ID config must match <provider>/<model>")]
    StrParse,

    /// Error when parsing `ProviderId`.
    #[error(transparent)]
    ProviderId(#[from] schematic::ConfigError),

    /// Error when parsing `ModelId`.
    #[error(transparent)]
    ModelId(#[from] ModelIdError),
}

impl FromStr for ModelIdConfig {
    type Err = ModelIdConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider, id) = s
            .split_once('/')
            .map(|(p, n)| (p.trim(), n.trim()))
            .ok_or(ModelIdConfigError::StrParse)?;

        Ok(Self {
            provider: ProviderId::from_str(provider)?,
            name: Name::from_str(id)?,
        })
    }
}

impl FromStr for PartialModelIdConfig {
    type Err = ModelIdConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (provider, name) = s
            .split_once('/')
            .map(|(p, n)| (p.trim(), n.trim()))
            .ok_or(ModelIdConfigError::StrParse)?;

        Ok(Self {
            provider: Some(ProviderId::from_str(provider)?),
            name: Some(Name::from_str(name)?),
        })
    }
}

/// The list of supported providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    #[default]
    /// Anthropic provider. See: <https://www.anthropic.com/api>.
    Anthropic,
    /// Deepseek provider. See: <https://api-docs.deepseek.com>. UNIMPLEMENTED.
    Deepseek,
    /// Google Gemini provider. See: <https://ai.google.dev/gemini-api/docs>.
    Google,
    /// Llama.cpp provider. See: <https://github.com/ggml-org/llama.cpp>.
    Llamacpp,
    /// Ollama provider. See: <https://ollama.com>.
    Ollama,
    /// Openai provider. See: <https://openai.com/api/>.
    Openai,
    /// Openrouter provider. See: <https://openrouter.io>.
    Openrouter,
    /// xAI provider. See: <https://x.ai/api>. UNIMPLEMENTED.
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

/// A model ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(try_from = "String")]
pub struct Name(pub String);

impl std::ops::Deref for Name {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<String> for Name {
    type Error = ModelIdError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::from_str(id.as_str())
    }
}

impl TryFrom<&str> for Name {
    type Error = ModelIdError;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        Self::from_str(id)
    }
}

impl FromStr for Name {
    type Err = ModelIdError;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        if id.is_empty()
            || id.chars().any(|c| {
                !(c.is_numeric()
                    || c.is_ascii_alphabetic()
                    || c == '-'
                    || c == '_'
                    || c == '.'
                    || c == ':'
                    || c == '/')
            })
        {
            return Err(ModelIdError);
        }

        Ok(Self(id.to_owned()))
    }
}

impl From<Name> for String {
    fn from(id: Name) -> Self {
        id.to_string()
    }
}

/// Error when parsing `ModelId`.
#[derive(Debug, thiserror::Error)]
#[error("Model ID must be [a-zA-Z0-9_-.:/]+")]
pub struct ModelIdError;
