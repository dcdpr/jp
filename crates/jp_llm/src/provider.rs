// pub mod deepseek;
pub mod google;
// pub mod xai;
pub mod anthropic;
pub mod llamacpp;
pub mod mock;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use anthropic::Anthropic;
use async_trait::async_trait;
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
pub trait Provider: std::fmt::Debug + Send + Sync {
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

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
