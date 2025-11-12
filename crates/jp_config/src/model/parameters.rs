//! LLM model parameters configuration.

use std::{fmt, str::FromStr};

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial, delta_opt_vec},
    partial::{ToPartial, partial_opt, partial_opt_config, partial_opts},
};

/// Assistant-specific configuration.
#[derive(Debug, Clone, Config)]
#[config(default, rename_all = "snake_case", allow_unknown_fields)]
pub struct ParametersConfig {
    /// Maximum number of tokens to generate.
    ///
    /// This can usually be left unset, in which case the model will be allowed
    /// to generate as many tokens as it supports. However, some providers,
    /// especially local ones such as `Ollama`, may set a very low token limit
    /// based on the local machine's resources, in such cases, it might be
    /// necessary to set a higher limit if your conversation is long or has more
    /// context attached.
    ///
    /// If unset, some providers may use "request chaining" to allow the model
    /// to generate more tokens than the maximum token limit of the model, if
    /// the model had not finished its complete response after the first
    /// request. You can either set `chain_on_max_tokens` to `false` for a given
    /// provider, or explicitly set this `max_tokens` parameter to a specific
    /// value to avoid request chaining. While request chaining is generally
    /// useful, sometimes you might want to have more tighter cost controls.
    pub max_tokens: Option<u32>,

    /// Reasoning configuration.
    ///
    /// If `None`, the model uses reasoning with reasonable defaults if it
    /// supports it, otherwise disabled. If set to `Some`, the model uses the
    /// provided configuration.
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

impl PartialConfigDelta for PartialParametersConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            max_tokens: delta_opt(self.max_tokens.as_ref(), next.max_tokens),
            reasoning: delta_opt_partial(self.reasoning.as_ref(), next.reasoning),
            temperature: delta_opt(self.temperature.as_ref(), next.temperature),
            top_p: delta_opt(self.top_p.as_ref(), next.top_p),
            top_k: delta_opt(self.top_k.as_ref(), next.top_k),
            stop_words: delta_opt_vec(self.stop_words.as_ref(), next.stop_words),
            other: delta_opt(self.other.as_ref(), next.other),
        }
    }
}

impl ToPartial for ParametersConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            max_tokens: partial_opts(self.max_tokens.as_ref(), None),
            reasoning: partial_opt_config(self.reasoning.as_ref(), None),
            temperature: partial_opts(self.temperature.as_ref(), None),
            top_p: partial_opts(self.top_p.as_ref(), None),
            top_k: partial_opts(self.top_k.as_ref(), None),
            stop_words: partial_opt(&self.stop_words, None),
            other: partial_opt(&self.other, None),
        }
    }
}

/// Define the name to serialize and deserialize for a unit variant.
mod strings {
    use crate::named_unit_variant;

    named_unit_variant!(off);
    named_unit_variant!(auto);
}

/// Reasoning configuration.
#[derive(Debug, Clone, Copy, Config)]
#[config(serde(untagged))]
pub enum ReasoningConfig {
    /// Reasoning is disabled, regardless of the model's capabilities.
    #[setting(with = "strings::off")]
    Off,

    /// Reasoning is enabled with reasonable defaults if the model supports it,
    /// otherwise disabled.
    #[setting(default, with = "strings::auto")]
    Auto,

    /// Reasoning is enabled with custom configuration, unless the model is
    /// known to not support reasoning.
    #[setting(nested)]
    Custom(CustomReasoningConfig),
}

impl AssignKeyValue for PartialReasoningConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        #[expect(clippy::single_match_else)]
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => {
                let mut custom = PartialCustomReasoningConfig::default();
                custom.assign(kv)?;
                *self = Self::Custom(custom);
            }
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialReasoningConfig {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Custom(prev), Self::Custom(next)) => Self::Custom(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for ReasoningConfig {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::Off => Self::Partial::Off,
            Self::Auto => Self::Partial::Auto,
            Self::Custom(v) => Self::Partial::Custom(v.to_partial()),
        }
    }
}

impl FromStr for PartialReasoningConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "off" => Self::Off,
            "auto" => Self::Auto,
            _ => Self::Custom(PartialCustomReasoningConfig::from_str(s)?),
        })
    }
}

impl FromStr for ReasoningConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let partial = PartialReasoningConfig::from_str(s)?;
        Self::from_partial(partial).map_err(Into::into)
    }
}

/// Custom reasoning configuration.
#[derive(Debug, Clone, Copy, PartialEq, Config)]
pub struct CustomReasoningConfig {
    /// Effort to use for reasoning.
    #[setting(default)]
    pub effort: ReasoningEffort,

    /// Whether to exclude reasoning tokens from the response. The model will
    /// still generate reasoning tokens, but they will not be included in the
    /// response.
    #[setting(default)]
    pub exclude: bool,
}

impl AssignKeyValue for PartialCustomReasoningConfig {
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

impl PartialConfigDelta for PartialCustomReasoningConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            effort: delta_opt(self.effort.as_ref(), next.effort),
            exclude: delta_opt(self.exclude.as_ref(), next.exclude),
        }
    }
}

impl ToPartial for CustomReasoningConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            effort: partial_opt(&self.effort, None),
            exclude: partial_opt(&self.exclude, None),
        }
    }
}

impl FromStr for PartialCustomReasoningConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            effort: Some(s.parse()?),
            exclude: None,
        })
    }
}

impl From<CustomReasoningConfig> for ReasoningConfig {
    fn from(config: CustomReasoningConfig) -> Self {
        Self::Custom(config)
    }
}

impl From<PartialCustomReasoningConfig> for PartialReasoningConfig {
    fn from(config: PartialCustomReasoningConfig) -> Self {
        Self::Custom(config)
    }
}

/// Effort to use for reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// Allows the model to decide the effort to use. If the model does not
    /// support auto-mode, it will fall back to `Medium`.
    Auto,

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
            Self::Auto | Self::Medium => max_tokens.saturating_mul(50) / 100,
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

impl From<u32> for Tokens {
    fn from(v: u32) -> Self {
        Self(v)
    }
}
