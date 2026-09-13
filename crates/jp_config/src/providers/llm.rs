//! LLM provider configurations.

pub mod anthropic;
pub mod cerebras;
pub mod deepseek;
pub mod google;
pub mod llamacpp;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use std::{collections::HashSet, fmt, str::FromStr};

use indexmap::IndexMap;
use schematic::{Config, ConfigError, HandlerError, Schema, SchemaBuilder, Schematic};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_map, path},
    fill::{FillDefaults, fill_map},
    model::id::{ModelIdConfig, ModelIdConfigError, ModelIdOrAliasConfig, resolve_alias_chain},
    partial::ToPartial,
    providers::llm::{
        anthropic::{AnthropicConfig, PartialAnthropicConfig},
        cerebras::{CerebrasConfig, PartialCerebrasConfig},
        deepseek::{DeepseekConfig, PartialDeepseekConfig},
        google::{GoogleConfig, PartialGoogleConfig},
        llamacpp::{LlamacppConfig, PartialLlamacppConfig},
        ollama::{OllamaConfig, PartialOllamaConfig},
        openai::{OpenaiConfig, PartialOpenaiConfig},
        openrouter::{OpenrouterConfig, PartialOpenrouterConfig},
    },
    util::merge_nested_indexmap,
    validate::Validator,
};

/// Provider configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(default, rename_all = "snake_case")]
pub struct LlmProviderConfig {
    /// Short names for models.
    ///
    /// Each value is a full model ID (`provider/name`), a `{ provider, name }`
    /// table, or the name of another alias.
    /// Aliases may point at other aliases; the chain is resolved to a concrete
    /// model.
    ///
    /// ```toml
    /// [providers.llm.aliases]
    /// opus = "anthropic/claude-opus-4"
    /// haiku = { provider = "anthropic", name = "claude-haiku-4-5" }
    /// coder = "opus"
    /// ```
    #[setting(nested, merge = merge_nested_indexmap)]
    pub aliases: IndexMap<String, ModelIdOrAliasConfig>,

    /// Anthropic API configuration.
    #[setting(nested)]
    pub anthropic: AnthropicConfig,

    /// Cerebras API configuration.
    #[setting(nested)]
    pub cerebras: CerebrasConfig,

    /// Deepseek API configuration.
    #[setting(nested)]
    pub deepseek: DeepseekConfig,

    /// Google API configuration.
    #[setting(nested)]
    pub google: GoogleConfig,

    /// Llamacpp API configuration.
    #[setting(nested)]
    pub llamacpp: LlamacppConfig,

    /// Ollama API configuration.
    #[setting(nested)]
    pub ollama: OllamaConfig,

    /// Openai API configuration.
    #[setting(nested)]
    pub openai: OpenaiConfig,

    /// Openrouter API configuration.
    #[setting(nested)]
    pub openrouter: OpenrouterConfig,
}

impl Validator for LlmProviderConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        self.anthropic.validate()?;
        self.cerebras.validate()?;
        self.deepseek.validate()?;
        self.google.validate()?;
        self.openai.validate()?;
        self.openrouter.validate()
    }
}

/// A single entry in a provider's `auth` credential chain.
///
/// Each entry names how the request is billed, and optionally which of your
/// credentials of that kind to use:
///
/// - `api_key`: Metered, per-token billing, using the sole key `api_key_env`
///   names.
/// - `api_key:<name>`: Metered billing with the named key, when `api_key_env`
///   maps several.
/// - `subscription`: A fixed-price plan's allowance, using the sole stored
///   credential.
/// - `subscription:<name>`: The named stored credential.
///
/// A name on its own selects whichever credential answers to it, of either
/// kind; two credentials sharing a name is a resolution error naming both.
///
/// A kind with no name resolves the sole credential of that kind, and reports
/// the candidates when there is more than one.
///
/// `api` and `sub` are accepted as shorthand for the two kinds, and are written
/// back in full.
/// Names are case-sensitive.
///
/// The grammar is shared across providers; which credentials exist and how an
/// entry resolves is each provider's own concern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AuthEntry {
    /// Bill per token, against an API key held in the environment.
    ApiKey(Option<String>),

    /// Bill against a subscription's allowance, using a stored credential.
    Subscription(Option<String>),

    /// Whichever credential answers to this name, of either kind.
    ///
    /// Stays unresolved through config: keys come from the environment and
    /// subscriptions from the credential store, so only a provider sees both.
    Named(String),
}

impl AuthEntry {
    /// Reject an empty chain or one carrying a duplicate entry.
    ///
    /// `key` is the configuration path the chain lives at, named in the error
    /// so the message points at the key the user wrote.
    ///
    /// Unrecognized entries are rejected earlier, when the value is parsed.
    ///
    /// # Errors
    ///
    /// Returns an error when `chain` is empty or contains the same entry twice.
    pub fn validate_chain(chain: &[Self], key: &str) -> Result<(), ConfigError> {
        if chain.is_empty() {
            return Err(HandlerError::new(format!(
                "{key} must contain at least one entry, e.g. [\"subscription\", \"api_key\"]"
            ))
            .into());
        }

        let mut seen = HashSet::new();
        for entry in chain {
            if !seen.insert(entry) {
                return Err(
                    HandlerError::new(format!("{key} contains duplicate entry {entry}")).into(),
                );
            }
        }

        Ok(())
    }

    /// The kind's canonical spelling, or `None` for [`Self::Named`].
    #[must_use]
    pub const fn kind(&self) -> Option<&'static str> {
        match self {
            Self::ApiKey(_) => Some("api_key"),
            Self::Subscription(_) => Some("subscription"),
            Self::Named(_) => None,
        }
    }

    /// Which credential the entry names, if it names one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::ApiKey(name) | Self::Subscription(name) => name.as_deref(),
            Self::Named(name) => Some(name),
        }
    }

    /// Whether resolving this entry could need the credential store.
    ///
    /// True for [`Self::Named`] as well as [`Self::Subscription`]: a bare name
    /// may turn out to be a subscription.
    #[must_use]
    pub const fn may_need_store(&self) -> bool {
        match self {
            Self::Subscription(_) | Self::Named(_) => true,
            Self::ApiKey(_) => false,
        }
    }
}

impl fmt::Display for AuthEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.kind(), self.name()) {
            (Some(kind), Some(name)) => write!(f, "{kind}:{name}"),
            (Some(kind), None) => f.write_str(kind),
            (None, Some(name)) => f.write_str(name),
            (None, None) => unreachable!("an entry without a kind names a credential"),
        }
    }
}

/// Error when parsing an [`AuthEntry`] from a string.
#[derive(Debug, thiserror::Error)]
#[error(
    "unrecognized auth chain entry: {0:?} (expected a credential name, \"api_key\", \
     \"subscription\", or \"<kind>:<name>\"; \"api\" and \"sub\" are accepted for the kinds)"
)]
pub struct AuthEntryParseError(String);

impl FromStr for AuthEntry {
    type Err = AuthEntryParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (kind, name) = match s.split_once(':') {
            Some((kind, name)) if !name.is_empty() => (kind, Some(name.to_owned())),
            // A trailing colon names nothing.
            Some(_) => return Err(AuthEntryParseError(s.to_owned())),
            None => (s, None),
        };

        match (kind, name) {
            ("api_key" | "api", name) => Ok(Self::ApiKey(name)),
            ("subscription" | "sub", name) => Ok(Self::Subscription(name)),

            // A bare word that is not a kind names a credential. A typo lands
            // here and is reported at resolution, which knows what exists.
            (name, None) if !name.is_empty() => Ok(Self::Named(name.to_owned())),

            _ => Err(AuthEntryParseError(s.to_owned())),
        }
    }
}

impl Serialize for AuthEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AuthEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl Schematic for AuthEntry {
    fn schema_name() -> Option<String> {
        Some("AuthEntry".into())
    }

    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        schema.string_default()
    }
}

impl AssignKeyValue for PartialLlmProviderConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("aliases") => kv.assign_to_entry(&mut self.aliases)?,
            _ if kv.p("anthropic") => self.anthropic.assign(kv)?,
            _ if kv.p("cerebras") => self.cerebras.assign(kv)?,
            _ if kv.p("deepseek") => self.deepseek.assign(kv)?,
            _ if kv.p("google") => self.google.assign(kv)?,
            _ if kv.p("llamacpp") => self.llamacpp.assign(kv)?,
            _ if kv.p("ollama") => self.ollama.assign(kv)?,
            _ if kv.p("openai") => self.openai.assign(kv)?,
            _ if kv.p("openrouter") => self.openrouter.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialLlmProviderConfig {
    // Not written in terms of `delta_with_unsets`: that method reports a value
    // whose correctness depends on the field being cleared first, so a caller
    // that drops the paths would merge a list onto the one already there.
    fn delta(&self, next: Self) -> Self {
        Self {
            aliases: delta_map(&self.aliases, next.aliases),
            anthropic: self.anthropic.delta(next.anthropic),
            cerebras: self.cerebras.delta(next.cerebras),
            deepseek: self.deepseek.delta(next.deepseek),
            google: self.google.delta(next.google),
            llamacpp: self.llamacpp.delta(next.llamacpp),
            ollama: self.ollama.delta(next.ollama),
            openai: self.openai.delta(next.openai),
            openrouter: self.openrouter.delta(next.openrouter),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            aliases: delta_map(&self.aliases, next.aliases),
            anthropic: self.anthropic.delta_with_unsets(
                next.anthropic,
                &path(prefix, "anthropic"),
                unsets,
            ),
            cerebras: self.cerebras.delta(next.cerebras),
            deepseek: self.deepseek.delta(next.deepseek),
            google: self.google.delta(next.google),
            llamacpp: self.llamacpp.delta(next.llamacpp),
            ollama: self.ollama.delta(next.ollama),
            openai: self.openai.delta(next.openai),
            openrouter: self.openrouter.delta_with_unsets(
                next.openrouter,
                &path(prefix, "openrouter"),
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialLlmProviderConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            aliases: fill_map(self.aliases, defaults.aliases),
            anthropic: self.anthropic.fill_from(defaults.anthropic),
            cerebras: self.cerebras.fill_from(defaults.cerebras),
            deepseek: self.deepseek.fill_from(defaults.deepseek),
            google: self.google.fill_from(defaults.google),
            llamacpp: self.llamacpp.fill_from(defaults.llamacpp),
            ollama: self.ollama.fill_from(defaults.ollama),
            openai: self.openai.fill_from(defaults.openai),
            openrouter: self.openrouter.fill_from(defaults.openrouter),
        }
    }
}

impl ToPartial for LlmProviderConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            aliases: self
                .aliases
                .iter()
                .map(|(k, v)| (k.clone(), v.to_partial()))
                .collect(),
            anthropic: self.anthropic.to_partial(),
            cerebras: self.cerebras.to_partial(),
            deepseek: self.deepseek.to_partial(),
            google: self.google.to_partial(),
            llamacpp: self.llamacpp.to_partial(),
            ollama: self.ollama.to_partial(),
            openai: self.openai.to_partial(),
            openrouter: self.openrouter.to_partial(),
        }
    }
}

impl LlmProviderConfig {
    /// Resolve every alias to a concrete [`ModelIdConfig`], following
    /// alias-to-alias chains.
    ///
    /// # Errors
    ///
    /// Returns an error if any alias chain contains a cycle, or ends in a name
    /// that is neither another alias nor a valid `provider/name` model ID.
    pub fn resolved_aliases(&self) -> Result<IndexMap<String, ModelIdConfig>, ModelIdConfigError> {
        self.aliases
            .keys()
            .map(|name| Ok((name.clone(), resolve_alias_chain(name, &self.aliases)?)))
            .collect()
    }
}

#[cfg(test)]
#[path = "llm_tests.rs"]
mod tests;
