//! LLM provider configurations.

pub mod anthropic;
pub mod cerebras;
pub mod deepseek;
pub mod google;
pub mod llamacpp;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use indexmap::IndexMap;
use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_mergeable_map, path},
    fill::{FillDefaults, fill_map},
    internal::merge::map_with_strategy,
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
    types::map::{MergeableMap, map_to_partial_per_key},
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
    ///
    /// Entries merge by key, so an alias defined in a later layer joins the
    /// ones an earlier layer set.
    /// Declare the map as `{ value = { … }, strategy = "replace" }` to drop
    /// them instead.
    #[setting(nested, merge = map_with_strategy)]
    pub aliases: MergeableMap<ModelIdOrAliasConfig>,

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
            aliases: delta_mergeable_map(&self.aliases, next.aliases),
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
            // The map states its own strategy, so a removed alias travels in
            // the value as a `replace` and needs no path reported.
            aliases: delta_mergeable_map(&self.aliases, next.aliases),
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
            // Key by key, so an alias only the defaults declare is added
            // while one this layer already has keeps its own value. A map
            // that states a strategy is left alone.
            aliases: match self.aliases {
                merged @ MergeableMap::Merged(_) => merged,
                MergeableMap::Map(entries) => {
                    fill_map(entries, defaults.aliases.into_map()).into()
                }
            },
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
            // Per key rather than `replace`: an alias the workspace config
            // gained after this conversation was created still reaches it.
            aliases: map_to_partial_per_key(self.aliases.iter()),
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
