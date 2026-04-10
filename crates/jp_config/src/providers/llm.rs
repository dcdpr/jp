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
    delta::PartialConfigDelta,
    fill::FillDefaults,
    model::id::ModelIdConfig,
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
};

/// Provider configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(default, rename_all = "snake_case")]
pub struct LlmProviderConfig {
    /// Aliases for specific provider/model combinations.
    ///
    /// This allows you to define short names for models.
    ///
    /// For example:
    ///
    /// ```toml
    /// [providers.llm.aliases]
    /// haiku = { provider = "anthropic", name = "claude-3-haiku-20240307" }
    /// ```
    #[setting(nested, merge = merge_nested_indexmap)]
    pub aliases: IndexMap<String, ModelIdConfig>,

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
            _ if kv.p("aliases") => kv.try_object()?,
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
    fn delta(&self, next: Self) -> Self {
        Self {
            aliases: next
                .aliases
                .into_iter()
                .filter_map(|(k, next)| {
                    let prev = self.aliases.get(&k);
                    if prev.is_some_and(|prev| prev == &next) {
                        return None;
                    }

                    let next = match prev {
                        Some(prev) => prev.delta(next),
                        None => next,
                    };

                    Some((k, next))
                })
                .collect(),
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
}

impl FillDefaults for PartialLlmProviderConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            aliases: self.aliases,
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

#[cfg(test)]
#[path = "llm_tests.rs"]
mod tests;
