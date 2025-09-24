//! LLM provider configurations.

pub mod anthropic;
pub mod deepseek;
pub mod google;
pub mod llamacpp;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    providers::llm::{
        anthropic::{AnthropicConfig, PartialAnthropicConfig},
        deepseek::{DeepseekConfig, PartialDeepseekConfig},
        google::{GoogleConfig, PartialGoogleConfig},
        llamacpp::{LlamacppConfig, PartialLlamacppConfig},
        ollama::{OllamaConfig, PartialOllamaConfig},
        openai::{OpenaiConfig, PartialOpenaiConfig},
        openrouter::{OpenrouterConfig, PartialOpenrouterConfig},
    },
};

/// Provider configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct LlmProviderConfig {
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

#[cfg(test)]
mod tests {
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
