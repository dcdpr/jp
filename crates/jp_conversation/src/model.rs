use std::{collections::HashMap, fmt, str::FromStr};

use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id,
};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const DEFAULT_SLUG: &str = "anthropic/claude-3.7-sonnet";

/// Structured representation of LLM model configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    #[serde(skip)]
    pub provider: ProviderId,

    pub slug: String,

    /// Maximum number of tokens to generate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Reasoning configuration.
    ///
    /// Should be `None` if the model does not support reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_words: Vec<String>,

    // Parameters specific to certain providers/models
    #[serde(default, flatten, skip_serializing_if = "HashMap::is_empty")]
    pub additional_parameters: HashMap<String, serde_json::Value>,
}

impl Model {
    #[must_use]
    pub fn new(provider: ProviderId, slug: impl Into<String>) -> Self {
        Self {
            provider,
            slug: slug.into(),
            ..Default::default()
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self {
            provider: ProviderId::default(),
            slug: DEFAULT_SLUG.to_string(),
            max_tokens: None,
            reasoning: None,
            temperature: None,
            stop_words: vec![],
            additional_parameters: HashMap::new(),
        }
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
}

/// Reference to an LLM model, either inline or by ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelReference {
    Inline(Model),
    Ref(ModelId),
}

impl Default for ModelReference {
    fn default() -> Self {
        Self::Inline(Model::default())
    }
}

/// ID wrapper for LLM Model
///
/// This is used for storage and display purposes, it is **NOT** the same as
/// [`Model::slug`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(String);

impl ModelId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn to_filename(&self) -> String {
        format!("{}.json", self.target_id())
    }

    // FIXME:
    //
    // Since models are stored in `<provider>/<slug>.json` files, basing the
    // model ID on the file name alone is not sufficient, as multiple providers
    // may have the same model file name.
    //
    // For example, the file `claude-3.7-sonnet.json` could live in
    // `anthropic/claude-3.7-sonnet.json` or in
    // `openrouter/claude-3.7-sonnet.json`.
    pub fn from_filename(filename: &str) -> Result<Self> {
        filename
            .strip_suffix(".json")
            .ok_or_else(|| Error::InvalidIdFormat(format!("Invalid model filename: {filename}")))
            .and_then(ModelId::try_from)
    }
}

impl Id for ModelId {
    fn variant() -> Variant {
        'm'.into()
    }

    fn target_id(&self) -> TargetId {
        self.0.clone().into()
    }

    fn global_id(&self) -> GlobalId {
        jp_id::global::get().into()
    }

    fn is_valid(&self) -> bool {
        Self::variant().is_valid() && self.global_id().is_valid()
    }
}

impl Default for ModelId {
    fn default() -> Self {
        Self(DEFAULT_SLUG.to_owned())
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format_id(f)
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
        if s.chars().any(|c| {
            !(c.is_numeric()
                || (c.is_ascii_alphabetic() && c.is_ascii_lowercase())
                || c == '-'
                || c == '_'
                || c == '/'
                || c == '.')
        }) {
            return Err(Error::InvalidIdFormat(
                "Model ID must be [a-z0-9_-/]".to_string(),
            ));
        }

        Ok(Self(s))
    }
}

impl FromStr for ModelId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        jp_id::parse::<Self>(s)
            .map(|p| Self(p.target_id.to_string()))
            .map_err(Into::into)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Anthropic,
    Deepseek,
    Google,
    Openai,
    #[default]
    Openrouter,
}

impl ProviderId {
    #[must_use]
    pub fn is_anthropic(&self) -> bool {
        matches!(self, Self::Anthropic)
    }

    #[must_use]
    pub fn is_deepseek(&self) -> bool {
        matches!(self, Self::Deepseek)
    }

    #[must_use]
    pub fn is_google(&self) -> bool {
        matches!(self, Self::Google)
    }

    #[must_use]
    pub fn is_openai(&self) -> bool {
        matches!(self, Self::Openai)
    }

    #[must_use]
    pub fn is_openrouter(&self) -> bool {
        matches!(self, Self::Openrouter)
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
            _ => Err(Error::InvalidProviderId(s.to_owned())),
        }
    }
}
