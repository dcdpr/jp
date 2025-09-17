//! LLM model parameters configuration.

use std::fmt;

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    BoxedError,
};

/// Assistant-specific configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case", allow_unknown_fields)]
pub struct ParametersConfig {
    /// Maximum number of tokens to generate.
    ///
    /// This can usually be left unset, in which case the model will be allowed
    /// to generate as many tokens as it supports. However, some providers,
    /// especially local ones such as `Ollama`, may set a very low token limit
    /// based on the local machine's resources, in such cases, it might be
    /// necessary to set a higher limit if your conversation is long or has more
    /// context attached.
    pub max_tokens: Option<u32>,

    /// Reasoning configuration.
    ///
    /// Should be `None` if the model does not support reasoning.
    #[setting(nested)]
    pub reasoning: Option<ReasoningConfig>,

    /// Temperature of the model.
    ///
    /// ...
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
    pub top_p: Option<f32>,

    /// Control the diversity and focus of the model's output. It determines how
    /// many of the most likely tokens (words or subwords) the model should
    /// consider when generating a response.
    ///
    /// As opposed to `top_p`, it is a fixed-size approach that considers the
    /// top K most probable tokens, discarding the rest.
    pub top_k: Option<u32>,

    /// The `stop_words` parameter can be set to specific sequences, such as a
    /// period or specific word, to stop the model from generating text when it
    /// encounters these sequences.
    #[setting(default, merge = schematic::merge::append_vec)]
    pub stop_words: Vec<String>,

    /// Other non-typed parameters that some models might support.
    #[setting(default, flatten, merge = schematic::merge::merge_iter)]
    pub other: Map<String, Value>,
}

impl AssignKeyValue for PartialParametersConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "max_tokens" => self.max_tokens = kv.try_some_u32()?,
            "temperature" => self.temperature = kv.try_some_f32()?,
            "top_p" => self.top_p = kv.try_some_f32()?,
            "top_k" => self.top_k = kv.try_some_u32()?,
            _ if kv.p("stop_words") => kv.try_some_vec_of_strings(&mut self.stop_words)?,
            _ if kv.p("reasoning") => self.reasoning.assign(kv)?,
            k => {
                self.other
                    .get_or_insert_default()
                    .insert(k.to_owned(), kv.value.into_value());
            }
        }

        Ok(())
    }
}

/// Configuration for reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Config)]
pub struct ReasoningConfig {
    /// Effort to use for reasoning.
    #[setting(default)]
    pub effort: ReasoningEffort,

    /// Whether to exclude reasoning tokens from the response. The model will
    /// still generate reasoning tokens, but they will not be included in the
    /// response.
    #[setting(default)]
    pub exclude: bool,
}

impl AssignKeyValue for PartialReasoningConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "effort" => self.effort = kv.try_some_from_str()?,
            "exclude" => self.exclude = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

/// Effort to use for reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
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
    #[variant(fallback)]
    Absolute(Tokens),
}

impl ReasoningEffort {
    /// Convert the effort to the absolute number of tokens to use.
    #[must_use]
    pub const fn to_tokens(self, max_tokens: u32) -> u32 {
        match self {
            Self::High => max_tokens.saturating_mul(80) / 100,
            Self::Medium => max_tokens.saturating_mul(50) / 100,
            Self::Low => max_tokens.saturating_mul(20) / 100,
            Self::Absolute(Tokens(tokens)) => tokens,
        }
    }

    /// Convert the effort to a relative effort, based on the given maximum
    /// number of tokens.
    #[must_use]
    pub const fn abs_to_rel(&self, max_tokens: Option<u32>) -> Self {
        match (self, max_tokens) {
            (Self::Absolute(Tokens(tokens)), Some(max)) => {
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

/// Wrapper around a number of tokens.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Tokens(u32);

impl fmt::Display for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<&str> for Tokens {
    type Error = BoxedError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Ok(Self(s.parse()?))
    }
}
