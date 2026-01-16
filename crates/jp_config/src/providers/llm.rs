//! LLM provider configurations.

pub mod anthropic;
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
    model::id::ModelIdConfig,
    partial::ToPartial,
    providers::llm::{
        anthropic::{AnthropicConfig, PartialAnthropicConfig},
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
    #[setting(nested, merge = merge_nested_indexmap)]
    pub aliases: IndexMap<String, ModelIdConfig>,

    /// Anthropic API configuration.
    #[setting(nested)]
    pub anthropic: AnthropicConfig,

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
            "" => *self = kv.try_object()?,
            _ if kv.p("aliases") => kv.try_object()?,
            _ if kv.p("anthropic") => self.anthropic.assign(kv)?,
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
            deepseek: self.deepseek.delta(next.deepseek),
            google: self.google.delta(next.google),
            llamacpp: self.llamacpp.delta(next.llamacpp),
            ollama: self.ollama.delta(next.ollama),
            openai: self.openai.delta(next.openai),
            openrouter: self.openrouter.delta(next.openrouter),
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
mod tests {
    use test_log::test;

    use super::*;
    use crate::assignment::KvAssignment;

    #[test]
    fn test_provider_config_anthropic() {
        let mut p = PartialLlmProviderConfig::default();

        let kv = KvAssignment::try_from_cli("anthropic.api_key_env", "MY_ANTHROPIC_KEY").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.anthropic.api_key_env.as_deref(), Some("MY_ANTHROPIC_KEY"));
    }

    #[test]
    fn test_provider_config_openai() {
        let mut p = PartialLlmProviderConfig::default();

        let kv =
            KvAssignment::try_from_cli("openai.base_url", "https://custom.openai.com").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.openai.base_url.as_deref(),
            Some("https://custom.openai.com")
        );
    }

    #[test]
    fn test_provider_config_openrouter_referrer() {
        let mut p = PartialLlmProviderConfig::default();

        let kv = KvAssignment::try_from_cli("openrouter.app_referrer", "").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.openrouter.app_referrer, Some(String::new()));

        let kv =
            KvAssignment::try_from_cli("openrouter.app_referrer", "https://example.com").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.openrouter.app_referrer,
            Some("https://example.com".to_string())
        );
    }
}
