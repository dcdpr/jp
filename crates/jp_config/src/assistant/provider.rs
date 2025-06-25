pub mod anthropic;
pub mod deepseek;
pub mod google;
pub mod llamacpp;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Provider configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Provider {
    /// Anthropic API configuration.
    #[config(nested)]
    pub anthropic: anthropic::Anthropic,

    /// Deepseek API configuration.
    #[config(nested)]
    pub deepseek: deepseek::Deepseek,

    /// Google API configuration.
    #[config(nested)]
    pub google: google::Google,

    /// Llamacpp API configuration.
    #[config(nested)]
    pub llamacpp: llamacpp::Llamacpp,

    /// Openrouter API configuration.
    #[config(nested)]
    pub openrouter: openrouter::Openrouter,

    /// Openai API configuration.
    #[config(nested)]
    pub openai: openai::Openai,

    /// Ollama API configuration.
    #[config(nested)]
    pub ollama: ollama::Ollama,
}

impl AssignKeyValue for <Provider as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            _ if kv.trim_prefix("anthropic") => self.anthropic.assign(kv)?,
            _ if kv.trim_prefix("deepseek") => self.deepseek.assign(kv)?,
            _ if kv.trim_prefix("llamacpp") => self.llamacpp.assign(kv)?,
            _ if kv.trim_prefix("google") => self.google.assign(kv)?,
            _ if kv.trim_prefix("openrouter") => self.openrouter.assign(kv)?,
            _ if kv.trim_prefix("openai") => self.openai.assign(kv)?,
            _ if kv.trim_prefix("ollama") => self.ollama.assign(kv)?,
            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
