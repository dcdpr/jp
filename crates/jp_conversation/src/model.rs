use std::{collections::HashMap, fmt, str::FromStr};

use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: ModelId,
    pub parameters: Parameters,
}

impl From<ModelId> for Model {
    fn from(id: ModelId) -> Self {
        Self {
            id,
            parameters: Parameters::default(),
        }
    }
}

/// Configuration for a model.
///
/// Note that not all models support all configuration options. Unsupported
/// options will be ignored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Parameters {
    /// Maximum number of tokens to generate.
    ///
    /// This can usually be left unset, in which case the model will be allowed
    /// to generate as many tokens as it supports. However, some providers,
    /// especially local ones such as `Ollama`, may set a very low token limit
    /// based on the local machine's resources, in such cases, it might be
    /// necessary to set a higher limit if your conversation is long or has more
    /// context attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Reasoning configuration.
    ///
    /// Should be `None` if the model does not support reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Control the randomness and diversity of the generated text. Also
    /// known as *nucleus sampling*.
    ///
    /// For example, if `top_p` is set to 0.8, the model will consider the top
    /// tokens whose cumulative probability just exceeds 0.8. This means the
    /// model will focus on the most probable options, making the output more
    /// controlled and less random.
    ///
    /// As opposed to `top_k`, this is a dynamic approach that considers tokens
    /// until their cumulative probability reaches a threshold P.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// Control the diversity and focus of the model's output. It determines how
    /// many of the most likely tokens (words or subwords) the model should
    /// consider when generating a response.
    ///
    /// As opposed to `top_p`, it is a fixed-size approach that considers the
    /// top K most probable tokens, discarding the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    /// The `stop_words` parameter can be set to specific sequences, such as a
    /// period or specific word, to stop the model from generating text when it
    /// encounters these sequences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_words: Vec<String>,

    // Other non-typed parameters that some models might support.
    #[serde(default, flatten, skip_serializing_if = "HashMap::is_empty")]
    pub other: HashMap<String, Value>,
}
impl Parameters {
    /// Merge `other` into `self`.
    pub fn merge(&mut self, mut other: Self) {
        self.max_tokens = other.max_tokens.or(self.max_tokens);
        self.reasoning = other.reasoning.or(self.reasoning);
        self.temperature = other.temperature.or(self.temperature);
        self.top_p = other.top_p.or(self.top_p);
        self.top_k = other.top_k.or(self.top_k);
        self.stop_words.append(&mut other.stop_words);
        self.other.extend(other.other);
    }

    /// Untyped setter for a parameter.
    pub fn set(&mut self, key: &str, value: String) -> std::result::Result<(), SetParameterError> {
        let error = SetParameterError::new(key, &value);
        match key {
            "max_tokens" => self.max_tokens = Some(value.parse().map_err(|e| error.with(e))?),
            "reasoning.effort" => {
                self.reasoning.get_or_insert_default().effort =
                    value.parse().map_err(|e| error.with(e))?;
            }
            "reasoning.exclude" => {
                self.reasoning.get_or_insert_default().exclude =
                    value.parse().map_err(|e| error.with(e))?;
            }
            "temperature" => self.temperature = Some(value.parse().map_err(|e| error.with(e))?),
            "top_p" => self.top_p = Some(value.parse().map_err(|e| error.with(e))?),
            "top_k" => self.top_k = Some(value.parse().map_err(|e| error.with(e))?),
            "stop_words" => self.stop_words = value.split(',').map(ToOwned::to_owned).collect(),
            _ => {
                self.other.insert(key.to_owned(), value.into());
            }
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Failed to set parameter `{key}` to `{value}`")]
pub struct SetParameterError {
    key: String,
    value: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl SetParameterError {
    #[must_use]
    fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            source: None,
        }
    }

    #[must_use]
    fn with(mut self, error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        self.source = Some(error.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Reasoning {
    /// Effort to use for reasoning.
    #[serde(default)]
    pub effort: ReasoningEffort,

    /// Whether to exclude reasoning tokens from the response. The model will
    /// still generate reasoning tokens, but they will not be included in the
    /// response.
    #[serde(default)]
    pub exclude: bool,
}

/// Effort to use for reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// Allocates a large portion of tokens for reasoning (approximately 80% of
    /// `max_tokens`)
    High,
    /// Allocates a moderate portion of tokens (approximately 50% of
    /// `max_tokens`)
    #[default]
    Medium,
    /// Allocates a smaller portion of tokens (approximately 20% of
    /// `max_tokens`)
    Low,

    /// Allocate a specific number of tokens for reasoning.
    Absolute(u32),
}

impl ReasoningEffort {
    #[must_use]
    pub fn to_tokens(self, max_tokens: u32) -> u32 {
        match self {
            Self::High => (max_tokens * 80) / 100,
            Self::Medium => (max_tokens * 50) / 100,
            Self::Low => (max_tokens * 20) / 100,
            Self::Absolute(tokens) => tokens,
        }
    }

    #[must_use]
    pub fn abs_to_rel(&self, max_tokens: Option<u32>) -> Self {
        match (self, max_tokens) {
            (Self::Absolute(tokens), Some(max)) => {
                if *tokens > (max * 80) / 100 {
                    Self::High
                } else if *tokens > (max * 50) / 100 {
                    Self::Medium
                } else {
                    Self::Low
                }
            }
            (Self::Absolute(_), None) => Self::Medium,
            (_, _) => *self,
        }
    }
}

impl FromStr for ReasoningEffort {
    type Err = Box<dyn std::error::Error + Send + Sync>;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "high" => Ok(Self::High),
            "medium" => Ok(Self::Medium),
            "low" => Ok(Self::Low),
            _ => Ok(Self::Absolute(s.parse().map_err(|_| {
                format!(
                    "Invalid reasoning effort: {s}, must be one of high, medium, low, or a number"
                )
            })?)),
        }
    }
}

/// ID wrapper for LLM Model
///
/// This is used for storage and display purposes, it is **NOT** the same as
/// [`Model::id`].
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
        let (provider, name) = s.split_once('/').unwrap_or(("", s));

        if name.chars().any(|c| {
            !(c.is_numeric()
                || (c.is_ascii_alphabetic() && c.is_ascii_lowercase())
                || c == '-'
                || c == '_'
                || c == '.'
                || c == ':'
                || c == '/')
        }) {
            return Err(Error::InvalidIdFormat(
                "Model ID must be [a-z0-9_-.:/]".to_string(),
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
            "openai" => Ok(Self::Openai),
            "openrouter" => Ok(Self::Openrouter),
            "ollama" => Ok(Self::Ollama),
            _ => Err(Error::InvalidProviderId(s.to_owned())),
        }
    }
}
