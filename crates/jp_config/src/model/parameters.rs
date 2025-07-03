use std::{collections::HashMap, str::FromStr};

use confique::Config as Confique;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
    serde::is_default,
};

/// Model configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
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
    #[config(default = [], partial_attr(serde(skip_serializing_if = "is_default")))]
    pub stop_words: Vec<String>,

    // Other non-typed parameters that some models might support.
    #[serde(default, flatten, skip_serializing_if = "HashMap::is_empty")]
    #[config(default = {}, partial_attr(serde(skip_serializing_if = "is_default")))]
    pub other: HashMap<String, Value>,
}

impl AssignKeyValue for <Parameters as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "max_tokens" => self.max_tokens = Some(kv.try_into_string()?.parse()?),

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
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
