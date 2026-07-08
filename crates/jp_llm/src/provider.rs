// pub mod deepseek;
pub mod google;
// pub mod xai;
pub mod anthropic;
pub mod cerebras;
pub mod llamacpp;
pub mod mock;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use std::sync::atomic::{AtomicU64, Ordering};

use anthropic::Anthropic;
use async_trait::async_trait;
use cerebras::Cerebras;
use google::Google;
use jp_config::{
    model::id::{Name, ProviderId},
    providers::llm::LlmProviderConfig,
};
use llamacpp::Llamacpp;
use ollama::Ollama;
use openai::Openai;
use openrouter::Openrouter;

use crate::{
    error::Result, model::ModelDetails, provider::mock::MockProvider, query::ChatQuery,
    stream::EventStream,
};

#[async_trait]
pub trait Provider: Send + Sync {
    /// Get details of a model.
    async fn model_details(&self, name: &Name) -> Result<ModelDetails>;

    /// Get a list of available models.
    async fn models(&self) -> Result<Vec<ModelDetails>>;

    /// Perform a streaming chat completion.
    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream>;
}

/// Get a provider by ID.
pub fn get_provider(id: ProviderId, config: &LlmProviderConfig) -> Result<Box<dyn Provider>> {
    let provider: Box<dyn Provider> = match id {
        ProviderId::Anthropic => Box::new(Anthropic::try_from(&config.anthropic)?),
        ProviderId::Cerebras => Box::new(Cerebras::try_from(&config.cerebras)?),
        ProviderId::Google => Box::new(Google::try_from(&config.google)?),
        ProviderId::Llamacpp => Box::new(Llamacpp::try_from(&config.llamacpp)?),
        ProviderId::Ollama => Box::new(Ollama::try_from(&config.ollama)?),
        ProviderId::Openai => Box::new(Openai::try_from(&config.openai)?),
        ProviderId::Openrouter => Box::new(Openrouter::try_from(&config.openrouter)?),

        ProviderId::Deepseek => todo!(),
        ProviderId::Xai => todo!(),

        ProviderId::Test => Box::new(MockProvider::new(vec![])),
    };

    Ok(provider)
}

/// Validate that a provider is able to accept requests: credentials present,
/// configuration well-formed.
///
/// Local and synchronous — performs no I/O. Constructing a provider implies
/// this check passes: this *is* [`get_provider`] with the client thrown away,
/// packaged as an explicit seam so callers can fail fast before starting
/// side-effectful work (spawning background tasks, loading attachments) that
/// is wasted when the request can never be sent.
///
/// # Errors
///
/// Returns the same errors as [`get_provider`], e.g.
/// [`Error::MissingEnv`](crate::Error::MissingEnv) when the provider's API
/// key environment variable is unset.
pub fn preflight(id: ProviderId, config: &LlmProviderConfig) -> Result<()> {
    get_provider(id, config).map(drop)
}

/// Build the provider-native chat request for `query` and serialize it to JSON,
/// without sending it.
///
/// Test-only seam for snapshotting request construction across providers,
/// notably the effect of conversation compaction on each provider's message
/// serialization.
/// Each arm runs the same builder the live path uses, so the snapshot reflects
/// what would go on the wire.
#[cfg(test)]
pub(crate) fn build_request_value(
    id: ProviderId,
    config: &LlmProviderConfig,
    model: &ModelDetails,
    query: ChatQuery,
) -> Result<serde_json::Value> {
    match id {
        ProviderId::Anthropic => {
            Anthropic::try_from(&config.anthropic)?.request_value(model, query)
        }
        ProviderId::Cerebras => Cerebras::try_from(&config.cerebras)?.request_value(model, query),
        ProviderId::Google => Google::try_from(&config.google)?.request_value(model, query),
        ProviderId::Llamacpp => Llamacpp::try_from(&config.llamacpp)?.request_value(model, query),
        ProviderId::Ollama => Ollama::try_from(&config.ollama)?.request_value(model, query),
        ProviderId::Openai => Openai::try_from(&config.openai)?.request_value(model, query),
        ProviderId::Openrouter => {
            Openrouter::try_from(&config.openrouter)?.request_value(model, query)
        }
        ProviderId::Test | ProviderId::Deepseek | ProviderId::Xai => {
            unreachable!("{id:?} is not part of the request snapshot suite")
        }
    }
}

/// Serialize a value to a temporary JSON file and return its path as a string.
///
/// Used by `trace!` fields to avoid dumping massive request payloads into the
/// log stream.
/// Each call writes a distinct, sequence-numbered file
/// (`{prefix}-{pid}-{seq}.json`) so successive requests within a single process
/// don't clobber each other's payloads.
pub(crate) fn trace_to_tmpfile(prefix: &str, value: &impl serde::Serialize) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{seq}.json", std::process::id()));
    match std::fs::write(
        &path,
        serde_json::to_string_pretty(value).unwrap_or_default(),
    ) {
        Ok(()) => path.display().to_string(),
        Err(_) => "<write failed>".to_owned(),
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "provider/compaction_request_tests.rs"]
mod compaction_request_tests;
