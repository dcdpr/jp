//! Mock provider for testing LLM interactions without real API calls.
//!
//! This module provides a configurable mock implementation of the [`Provider`]
//! trait, useful for:
//!
//! - Integration tests that need to simulate LLM responses
//! - Testing interrupt/signal handling during streaming
//! - Verifying persistence logic without network calls
//!
//! # Example
//!
//! ```ignore
//! use jp_llm::provider::mock::MockProvider;
//! use jp_llm::event::{Event, FinishReason};
//! use jp_conversation::event::{ConversationEvent, ChatResponse};
//!
//! // Create a provider that returns a simple message
//! let provider = MockProvider::with_message("Hello, world!");
//!
//! // Or create one with custom events for more complex scenarios
//! let provider = MockProvider::new(vec![
//!     Event::Part {
//!         index: 0,
//!         event: ConversationEvent::now(ChatResponse::message("Hello")),
//!     },
//!     Event::flush(0),
//!     Event::Finished(FinishReason::Completed),
//! ]);
//! ```

use async_trait::async_trait;
use futures::stream;
use jp_config::model::id::{ModelIdConfig, Name, ProviderId};
use jp_conversation::event::{ChatResponse, ToolCallRequest};
use serde_json::{Map, Value};

use super::Provider;
use crate::{
    error::Result,
    event::{Event, FinishReason},
    model::ModelDetails,
    query::ChatQuery,
    stream::EventStream,
};

/// A mock LLM provider for testing.
///
/// Returns predetermined events from [`chat_completion_stream`], allowing tests
/// to simulate various LLM behaviors without making real API calls.
///
/// [`chat_completion_stream`]: Provider::chat_completion_stream
#[derive(Debug, Clone)]
pub struct MockProvider {
    /// Events to return from the stream.
    events: Vec<Event>,

    /// Model details to return.
    model: ModelDetails,
}

impl MockProvider {
    /// Create a new mock provider with the given events.
    ///
    /// The events will be returned in order from [`chat_completion_stream`].
    ///
    /// [`chat_completion_stream`]: Provider::chat_completion_stream
    #[must_use]
    pub fn new(events: Vec<Event>) -> Self {
        Self {
            events,
            model: Self::default_model(),
        }
    }

    /// Create a mock provider that streams a simple message response.
    ///
    /// Useful for basic tests that just need some content to be streamed.
    #[must_use]
    pub fn with_message(content: &str) -> Self {
        Self::new(vec![
            Event::Part {
                index: 0,
                event: ChatResponse::message(content).into(),
            },
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ])
    }

    /// Create a mock provider that streams reasoning followed by a message.
    #[must_use]
    pub fn with_reasoning_and_message(reasoning: &str, message: &str) -> Self {
        Self::new(vec![
            Event::Part {
                index: 0,
                event: ChatResponse::reasoning(reasoning).into(),
            },
            Event::flush(0),
            Event::Part {
                index: 1,
                event: ChatResponse::message(message).into(),
            },
            Event::flush(1),
            Event::Finished(FinishReason::Completed),
        ])
    }

    /// Create a mock provider that streams content in multiple chunks.
    ///
    /// Useful for testing streaming behavior and partial content handling.
    #[must_use]
    pub fn with_chunked_message(chunks: &[&str]) -> Self {
        let mut events = Vec::with_capacity(chunks.len() + 2);

        for &chunk in chunks {
            events.push(Event::Part {
                index: 0,
                event: ChatResponse::message(chunk).into(),
            });
        }

        events.push(Event::flush(0));
        events.push(Event::Finished(FinishReason::Completed));

        Self::new(events)
    }

    /// Create a mock provider that requests a tool call.
    #[must_use]
    pub fn with_tool_call(
        tool_id: impl Into<String>,
        tool_name: impl Into<String>,
        arguments: Map<String, Value>,
    ) -> Self {
        Self::new(vec![
            Event::Part {
                index: 0,
                event: ToolCallRequest {
                    id: tool_id.into(),
                    name: tool_name.into(),
                    arguments,
                }
                .into(),
            },
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ])
    }

    /// Set custom model details for this provider.
    #[must_use]
    pub fn with_model(mut self, model: ModelDetails) -> Self {
        self.model = model;
        self
    }

    /// Set the model name.
    #[must_use]
    pub fn with_model_name(mut self, name: impl Into<String>) -> Self {
        self.model.id = Self::make_model_id(name);
        self
    }

    fn default_model() -> ModelDetails {
        ModelDetails::empty(Self::make_model_id("mock-model"))
    }

    fn make_model_id(name: impl Into<String>) -> ModelIdConfig {
        ModelIdConfig {
            provider: ProviderId::Test,
            name: name.into().parse().expect("valid model name"),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails> {
        let mut model = self.model.clone();
        model.id = ModelIdConfig {
            provider: ProviderId::Test,
            name: name.clone(),
        };
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>> {
        Ok(vec![self.model.clone()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream> {
        let events = self.events.clone();
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

#[cfg(test)]
#[path = "mock_tests.rs"]
mod tests;
