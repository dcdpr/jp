// pub mod deepseek;
pub mod google;
// pub mod xai;
pub mod anthropic;
pub mod llamacpp;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use std::{mem, pin::Pin};

use anthropic::Anthropic;
use async_trait::async_trait;
use futures::{Stream, StreamExt as _};
use google::Google;
use jp_config::{
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::{CustomReasoningConfig, ParametersConfig, ReasoningConfig, ReasoningEffort},
    },
    providers::llm::LlmProviderConfig,
};
use jp_conversation::event::{ChatResponse, ConversationEvent, ToolCallRequest};
use llamacpp::Llamacpp;
use ollama::Ollama;
use openai::Openai;
use openrouter::Openrouter;
use serde_json::Value;
use time::Date;
use tracing::warn;

use crate::{
    Error,
    error::Result,
    query::{ChatQuery, StructuredQuery},
    stream::{
        delta::Delta,
        event::{CompletionChunk, StreamEndReason, StreamEvent},
    },
    structured::SCHEMA_TOOL_NAME,
};

pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

/// Details about a model for a given provider, as specified by the provider.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDetails {
    /// The id of the model.
    pub id: ModelIdConfig,

    /// The display name of the model, if known.
    pub display_name: Option<String>,

    /// The context window size in tokens, if known.
    pub context_window: Option<u32>,

    /// The maximum output tokens, if known.
    pub max_output_tokens: Option<u32>,

    /// Whether the model supports reasoning, if unknown, this value is left to
    /// `None`.
    pub reasoning: Option<ReasoningDetails>,

    /// The knowledge cutoff date, if known.
    pub knowledge_cutoff: Option<Date>,

    /// Deprecation status of the model, if known.
    pub deprecated: Option<ModelDeprecation>,

    /// Provider-specific features.
    pub features: Vec<&'static str>,
}

impl ModelDetails {
    fn empty(id: ModelIdConfig) -> Self {
        Self {
            id,
            display_name: None,
            context_window: None,
            max_output_tokens: None,
            reasoning: None,
            knowledge_cutoff: None,
            deprecated: None,
            features: vec![],
        }
    }

    #[must_use]
    pub fn custom_reasoning_config(
        &self,
        config: Option<ReasoningConfig>,
    ) -> Option<CustomReasoningConfig> {
        match self.reasoning {
            // Unknown support
            None => match config {
                // Unconfigured or off, so disabled.
                None | Some(ReasoningConfig::Off) => None,

                // Auto configured, so use medium effort.
                Some(ReasoningConfig::Auto) => Some(CustomReasoningConfig {
                    effort: ReasoningEffort::Medium,
                    exclude: false,
                }),

                // Custom configuration, so use it.
                Some(ReasoningConfig::Custom(custom)) => Some(custom),
            },

            // Unsupported
            Some(ReasoningDetails::Unsupported) => match config {
                // Unconfigured, auto or off, so disabled.
                None | Some(ReasoningConfig::Auto | ReasoningConfig::Off) => None,

                // Custom configuration, invalid, so warn + disabled.
                Some(ReasoningConfig::Custom(config)) => {
                    warn!(
                        id = %self.id,
                        ?config,
                        "Model does not support reasoning, but the configuration explicitly \
                        enabled it. Reasoning will be disabled to avoid failed requests."
                    );

                    None
                }
            },

            // Supported
            Some(ReasoningDetails::Supported { .. }) => match config {
                // Off, so disabled.
                Some(ReasoningConfig::Off) => None,

                // Unconfigured, or auto, so medium effort.
                None | Some(ReasoningConfig::Auto) => Some(CustomReasoningConfig {
                    effort: ReasoningEffort::Medium,
                    exclude: false,
                }),

                // Custom configuration, so use it.
                Some(ReasoningConfig::Custom(custom)) => Some(custom),
            },
        }
    }
}

/// The deprecation status of a model.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ModelDeprecation {
    /// The model is active and available for use.
    #[default]
    Active,

    /// The model is deprecated and will be removed at some point in the future.
    Deprecated {
        /// Any details about the deprecation.
        ///
        /// This could include a link to the deprecation notice, a reason for
        /// deprecation, or recommended replacements.
        note: String,

        /// The date on which the model will be retired, if known.
        retire_at: Option<Date>,
    },
}

impl ModelDeprecation {
    pub fn deprecated(note: &impl ToString, retire_at: Option<Date>) -> Self {
        Self::Deprecated {
            note: note.to_string(),
            retire_at,
        }
    }
}

/// Details about the reasoning capabilities of a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReasoningDetails {
    Unsupported,
    Supported {
        /// The minimum number of reasoning tokens required to generate a
        /// response. Usually zero, but can be non-zero for certain models.
        min_tokens: u32,

        /// The maximum number of reasoning tokens that can be generated.
        max_tokens: Option<u32>,
    },
}

impl ReasoningDetails {
    #[must_use]
    pub fn supported(min_tokens: u32, max_tokens: Option<u32>) -> Self {
        Self::Supported {
            min_tokens,
            max_tokens,
        }
    }

    #[must_use]
    pub fn unsupported() -> Self {
        Self::Unsupported
    }

    #[must_use]
    pub fn min_tokens(&self) -> u32 {
        match self {
            Self::Supported { min_tokens, .. } => *min_tokens,
            Self::Unsupported => 0,
        }
    }

    #[must_use]
    pub fn max_tokens(&self) -> Option<u32> {
        match self {
            Self::Supported { max_tokens, .. } => *max_tokens,
            Self::Unsupported => None,
        }
    }
}

/// A collection of events in a single reply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    /// The provider that generated the reply.
    ///
    /// This is needed because certain events such as reasoning are interpreted
    /// differently between LLM providers, and some providers don't support
    /// reasoning from other models (e.g. Anthropic, which uses opaque
    /// signatures to validate reasoning).
    pub provider: ProviderId,
    events: Vec<Event>,
}

impl Reply {
    /// Returns the list of events in the reply.
    #[must_use]
    pub fn into_inner(self) -> Vec<Event> {
        self.events
    }
}

impl std::ops::Deref for Reply {
    type Target = Vec<Event>;

    fn deref(&self) -> &Self::Target {
        &self.events
    }
}

impl std::ops::DerefMut for Reply {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.events
    }
}

impl From<Reply> for Vec<ConversationEvent> {
    fn from(reply: Reply) -> Self {
        let mut events = vec![];

        for event in reply.events {
            match event {
                Event::Content(v) => events.push(ConversationEvent::now(ChatResponse::message(v))),
                Event::Reasoning(v) => {
                    events.push(ConversationEvent::now(ChatResponse::reasoning(v)));
                }
                Event::ToolCall(v) => events.push(ConversationEvent::now(v)),
                Event::Metadata(k, v) => {
                    if let Some(ChatResponse::Reasoning { metadata, .. }) =
                        events.last_mut().and_then(|e| e.as_chat_response_mut())
                    {
                        metadata.insert(k, v);
                    }
                }
                Event::Finished(_) => {}
            }
        }

        events
    }
}

impl From<(ProviderId, Vec<StreamEvent>)> for Reply {
    fn from((provider, stream_events): (ProviderId, Vec<StreamEvent>)) -> Self {
        let mut events: Vec<Event> = vec![];

        for event in stream_events {
            match event {
                StreamEvent::ChatChunk(chunk) => match chunk {
                    CompletionChunk::Content(text) => {
                        match events.last_mut().and_then(|e| e.as_content_mut()) {
                            Some(content) => content.push_str(&text),
                            None => events.push(Event::Content(text)),
                        }
                    }
                    CompletionChunk::Reasoning(text) => {
                        match events.last_mut().and_then(|e| e.as_reasoning_mut()) {
                            Some(reasoning) => reasoning.push_str(&text),
                            None => events.push(Event::Reasoning(text)),
                        }
                    }
                },
                StreamEvent::ToolCall(call) => {
                    events.push(Event::ToolCall(call));
                }
                StreamEvent::Metadata(key, metadata) => {
                    events.push(Event::Metadata(key, metadata));
                }
                StreamEvent::EndOfStream(reason) => events.push(Event::Finished(reason)),
            }
        }

        Self { provider, events }
    }
}

/// Represents a completed event from the LLM.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Chat response text
    Content(String),

    /// Reasoning response text
    Reasoning(String),

    /// A request to call a tool
    ToolCall(ToolCallRequest),

    /// Opaque provider-specific metadata.
    Metadata(String, Value),

    /// The reason the response was finished.
    Finished(StreamEndReason),
}

impl Event {
    #[must_use]
    pub fn metadata(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Metadata(key.into(), value.into())
    }

    #[must_use]
    pub fn as_content_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Content(content) => Some(content),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_reasoning_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Reasoning(reasoning) => Some(reasoning),
            _ => None,
        }
    }
}

impl From<Event> for StreamEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::Content(content) => StreamEvent::ChatChunk(CompletionChunk::Content(content)),
            Event::Reasoning(content) => {
                StreamEvent::ChatChunk(CompletionChunk::Reasoning(content))
            }
            Event::ToolCall(call) => StreamEvent::ToolCall(call),
            Event::Metadata(key, metadata) => StreamEvent::Metadata(key, metadata),
            Event::Finished(reason) => StreamEvent::EndOfStream(reason),
        }
    }
}

impl From<Delta> for Option<Result<Event>> {
    fn from(delta: Delta) -> Self {
        if let Some(content) = delta.content {
            return Some(Ok(Event::Content(content)));
        }

        if let Some(content) = delta.reasoning {
            return Some(Ok(Event::Reasoning(content)));
        }

        if let Some(args) = delta.tool_call_arguments {
            return Some(Ok(Event::ToolCall(ToolCallRequest {
                id: delta.tool_call_id.unwrap_or_default(),
                name: delta.tool_call_name.unwrap_or_default(),
                arguments: match serde_json::from_str(&args) {
                    Ok(arguments) => arguments,
                    Err(error) => return Some(Err(error.into())),
                },
            })));
        }

        None
    }
}

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
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<EventStream>;

    /// Perform a non-streaming chat completion.
    ///
    /// Default implementation collects results from the streaming version.
    async fn chat_completion(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<Reply> {
        let mut stream = self
            .chat_completion_stream(model, parameters, query)
            .await?;

        let mut events = Vec::new();
        let mut reasoning = String::new();
        let mut content = String::new();

        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::ChatChunk(chunk) => match chunk {
                    CompletionChunk::Content(text) => content.push_str(&text),
                    CompletionChunk::Reasoning(text) => reasoning.push_str(&text),
                },
                StreamEvent::ToolCall(call) => {
                    // We drain the buffers when we encounter a tool call to
                    // preserve the chronological ordering of events.
                    if !reasoning.is_empty() {
                        events.push(Event::Reasoning(mem::take(&mut reasoning)));
                    }
                    if !content.is_empty() {
                        events.push(Event::Content(mem::take(&mut content)));
                    }

                    events.push(Event::ToolCall(call));
                }
                StreamEvent::Metadata(key, metadata) => events.push(Event::Metadata(key, metadata)),
                StreamEvent::EndOfStream(reason) => events.push(Event::Finished(reason)),
            }
        }

        if !reasoning.is_empty() {
            events.push(Event::Reasoning(reasoning));
        }
        if !content.is_empty() {
            events.push(Event::Content(content));
        }

        Ok(Reply {
            provider: model.id.provider,
            events,
        })
    }

    /// Perform a structured completion.
    ///
    /// Default implementation uses a specialized tool-call to get structured
    /// results.
    ///
    /// Providers that have a dedicated structured response endpoint should
    /// override this method.
    async fn structured_completion(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: StructuredQuery,
    ) -> Result<Value> {
        let mut chat_query = ChatQuery {
            thread: query.thread.clone(),
            tools: vec![query.tool_definition()],
            tool_choice: query.tool_choice(),
            tool_call_strict_mode: true,
        };

        let max_retries = 3;
        for i in 1..=3 {
            let result = self
                .chat_completion(model, parameters, chat_query.clone())
                .await;
            let events = match result {
                Ok(events) => events,
                Err(error) if i >= max_retries => return Err(error),
                Err(error) => {
                    warn!(%error, "Error while getting structured data. Retrying in non-strict mode.");
                    chat_query.tool_call_strict_mode = false;
                    continue;
                }
            };

            let data = events
                .into_inner()
                .into_iter()
                .find_map(|event| match event {
                    Event::ToolCall(call) if call.name == SCHEMA_TOOL_NAME => Some(call.arguments),
                    _ => None,
                });

            match data {
                Some(data) => return Ok(query.map(Value::Object(data))),
                None if i >= max_retries => return Err(Error::MissingStructuredData),
                None => {
                    warn!("Failed to fetch structured data. Retrying.");
                }
            }
        }

        unreachable!();
    }
}

pub fn get_provider(id: ProviderId, config: &LlmProviderConfig) -> Result<Box<dyn Provider>> {
    let provider: Box<dyn Provider> = match id {
        ProviderId::Anthropic => Box::new(Anthropic::try_from(&config.anthropic)?),
        ProviderId::Deepseek => todo!(),
        ProviderId::Google => Box::new(Google::try_from(&config.google)?),
        ProviderId::Llamacpp => Box::new(Llamacpp::try_from(&config.llamacpp)?),
        ProviderId::Ollama => Box::new(Ollama::try_from(&config.ollama)?),
        ProviderId::Openai => Box::new(Openai::try_from(&config.openai)?),
        ProviderId::Openrouter => Box::new(Openrouter::try_from(&config.openrouter)?),
        ProviderId::Xai => todo!(),
    };

    Ok(provider)
}

#[derive(Default, Debug)]
/// A parser that segments a stream of text into 'reasoning' and 'other'
/// buckets. It handles streams with or without a `<think>` block.
pub struct ReasoningExtractor {
    pub other: String,
    pub reasoning: String,
    buffer: String,
    state: ReasoningState,
}

#[derive(Default, PartialEq, Debug)]
enum ReasoningState {
    #[default]
    /// The default state. Processing 'other' text while looking for
    /// `<think>\n`.
    Idle,
    /// Found `<think>\n`. Processing 'reasoning' text while looking for
    /// `</think>\n`.
    Accumulating,
    /// Found `</think>\n`. All subsequent text is 'other'.
    Finished,
}

impl ReasoningExtractor {
    /// Processes a chunk of the incoming text stream.
    pub fn handle(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        self.buffer.push_str(text);

        loop {
            match self.state {
                ReasoningState::Idle => {
                    if let Some(tag_start_index) = self.buffer.find("<think>\n") {
                        // Tag found. Text before it is 'other'.
                        self.other.push_str(&self.buffer[..tag_start_index]);

                        // Drain the processed 'other' text and the tag itself.
                        let tag_end_offset = tag_start_index + "<think>\n".len();
                        self.buffer.drain(..tag_end_offset);

                        // Transition state and re-process the rest of the
                        // buffer.
                        self.state = ReasoningState::Accumulating;
                    } else {
                        // No tag found. We can safely move most of the buffer
                        // to `other`, but must keep a small tail in case a tag
                        // is split across chunks.
                        let tail_len = self.buffer.len().min("<think>\n".len() - 1);
                        let mut drain_to = self.buffer.len() - tail_len;

                        if drain_to > 0 {
                            while !self.buffer.is_char_boundary(drain_to) {
                                drain_to += 1;
                            }

                            self.other.push_str(&self.buffer[..drain_to]);
                            self.buffer.drain(..drain_to);
                        }

                        // Wait for more data.
                        return;
                    }
                }
                ReasoningState::Accumulating => {
                    if let Some(tag_start_index) = self.buffer.find("</think>\n") {
                        // Closing tag found. Text before it is 'thinking'.
                        self.reasoning.push_str(&self.buffer[..tag_start_index]);

                        // Drain the 'reasoning' text and the tag.
                        let tag_end_offset = tag_start_index + "</think>\n".len();
                        self.buffer.drain(..tag_end_offset);

                        // Transition state and re-process.
                        self.state = ReasoningState::Finished;
                    } else {
                        // No closing tag found yet. Move "safe" part of the
                        // buffer to `reasoning`.
                        let tail_len = self.buffer.len().min("</think>\n".len() - 1);
                        let drain_to = self.buffer.len() - tail_len;

                        if drain_to > 0 {
                            self.reasoning.push_str(&self.buffer[..drain_to]);
                            self.buffer.drain(..drain_to);
                        }

                        // Wait for more data.
                        return;
                    }
                }
                ReasoningState::Finished => {
                    // Everything from now on is 'other'. No need for complex
                    // buffering.
                    self.other.push_str(&self.buffer);
                    self.buffer.clear();
                    return;
                }
            }
        }
    }

    /// Call this after the stream has finished to process any remaining data
    /// and fix potential unclosed thinking blocks.
    pub fn finalize(&mut self) {
        match self.state {
            ReasoningState::Accumulating => {
                let (reasoning, other) = self
                    .buffer
                    .split_once("</think>")
                    .unwrap_or((self.buffer.as_str(), ""));

                self.reasoning.push_str(reasoning);
                self.other.push_str(other);
            }
            _ => {
                self.other.push_str(&self.buffer);
            }
        }
        self.buffer.clear();
    }
}
