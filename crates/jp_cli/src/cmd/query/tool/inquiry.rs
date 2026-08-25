//! Structured inquiry support for tool execution.
//!
//! When a tool requires additional input with `QuestionTarget::Assistant`, the
//! `ToolCoordinator` spawns an async inquiry task that makes a structured
//! output request to the LLM, extracts the answer, and sends it back via the
//! event channel.
//! The tool is then re-executed with the answer.
//!
//! This module provides:
//!
//! - Schema generation and answer extraction
//! - The [`InquiryBackend`] trait for testability
//! - [`LlmInquiryBackend`] for real LLM calls
//! - `MockInquiryBackend` for tests
//!
//! See `docs/architecture/stateful-tool-inquiries.md` for the full design.

use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use jp_attachment::Attachment;
use jp_config::assistant::{sections::SectionConfig, tool_choice::ToolChoice};
use jp_conversation::{ConversationStream, event::ChatRequest, thread::Thread};
use jp_llm::{
    Provider,
    event_builder::structured_data,
    model::ModelDetails,
    query::ChatQuery,
    retry::{RetryConfig, collect_with_retry},
    tool::ToolDefinition,
    window,
};
use jp_tool::{AnswerType, Question};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Create the stream-correlation inquiry ID for a tool-call question.
///
/// Format: `<tool_call_id>.<question_id>.<attempt>`, unique per attempt within
/// the turn (`attempt` is the 1-indexed per-`(tool_call_id, question_id)`
/// counter from `TurnState`).
/// The tool name is not included because it's already stored in the
/// `InquirySource` of the `InquiryRequest` event.
pub fn tool_call_inquiry_id(tool_call_id: &str, question_id: &str, attempt: usize) -> String {
    format!("{tool_call_id}.{question_id}.{attempt}")
}

/// Create a JSON schema for a structured inquiry.
///
/// The schema is stable per answer type for prompt cache reuse.
pub fn create_inquiry_schema(question: &Question) -> Map<String, Value> {
    let answer_schema = match &question.answer_type {
        AnswerType::Boolean => json!({
            "type": "boolean"
        }),
        AnswerType::Select { options } => json!({
            "type": "string",
            "enum": options
        }),
        AnswerType::Text | AnswerType::Secret => json!({
            "type": "string"
        }),
    };

    Map::from_iter([
        ("type".into(), json!("object")),
        (
            "properties".into(),
            json!({
                "answer": answer_schema
            }),
        ),
        ("required".into(), json!(["answer"])),
        ("additionalProperties".into(), json!(false)),
    ])
}

/// Error type for inquiry operations.
#[derive(Debug, thiserror::Error)]
pub enum InquiryError {
    /// The LLM provider returned an error.
    #[error("LLM provider error: {0}")]
    Provider(#[from] jp_llm::Error),

    /// The inquiry was cancelled (e.g. Ctrl+C).
    #[error("Inquiry cancelled")]
    Cancelled,

    /// The LLM response did not contain structured data.
    #[error("No structured data in the LLM response")]
    MissingStructuredData,

    /// The structured response did not contain a valid answer.
    #[error("Failed to extract answer from structured response: {reason}")]
    AnswerExtraction {
        /// What went wrong during extraction.
        reason: String,
    },

    /// A catch-all for other errors (primarily used by mock backends).
    #[error("{0}")]
    #[allow(dead_code, reason = "used by MockInquiryBackend in tests")]
    Other(String),
}

/// Abstraction over how inquiries are resolved.
///
/// The real implementation ([`LlmInquiryBackend`]) makes a structured output
/// call to an LLM provider.
/// Tests use `MockInquiryBackend` to return pre-configured answers without
/// network calls.
#[async_trait]
pub trait InquiryBackend: Send + Sync {
    /// Resolve an inquiry and return the answer.
    ///
    /// `events` is an owned conversation stream (cloned just-in-time by the
    /// caller).
    /// The implementation appends temporary events, builds a thread, makes the
    /// LLM call, and extracts the answer.
    ///
    /// `inquiry_id` is an opaque correlation ID for logging.
    /// `tool_name` identifies the tool that needs input.
    /// `question` describes the expected answer type and text.
    async fn inquire(
        &self,
        events: ConversationStream,
        inquiry_id: &str,
        tool_name: &str,
        question: &Question,
        cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError>;
}

/// Fully-resolved configuration for a single inquiry call.
///
/// Built at turn start from the three-layer merge: per-question
/// `PartialAssistantConfig` → global inquiry config → main assistant config.
pub struct InquiryConfig {
    pub provider: Arc<dyn Provider>,
    pub model: ModelDetails,
    pub system_prompt: Option<String>,
    pub sections: Vec<SectionConfig>,

    /// Output ceiling for the inquiry request, from
    /// `assistant.request.max_response_bytes`.
    pub max_response_bytes: u32,
}

/// Resolves inquiries by making structured output calls to an LLM provider.
///
/// Holds a default [`InquiryConfig`] (from the global inquiry config merged
/// with the parent assistant config) and optional per-question overrides.
/// The parent turn's tool definitions and attachments are shared across all
/// inquiries.
pub struct LlmInquiryBackend {
    default_config: InquiryConfig,

    /// Per-question overrides keyed by `(tool_name, question_id)`.
    /// Built at turn start from `QuestionTarget::Assistant(config)` entries in
    /// the active tool configs.
    overrides: IndexMap<(String, String), InquiryConfig>,

    /// Attachments from the parent turn (shared across all inquiries).
    attachments: Vec<Attachment>,

    /// Tool definitions from the parent turn.
    ///
    /// Included in the inquiry request (with `ToolChoice::None`) so that the
    /// Anthropic prompt cache prefix matches the normal turn requests.
    tools: Vec<ToolDefinition>,
}

impl LlmInquiryBackend {
    #[must_use]
    pub fn new(
        default_config: InquiryConfig,
        overrides: IndexMap<(String, String), InquiryConfig>,
        attachments: Vec<Attachment>,
        tools: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            default_config,
            overrides,
            attachments,
            tools,
        }
    }

    /// Look up the effective config for this tool/question pair.
    ///
    /// Returns the per-question override when one exists, otherwise the default
    /// built from the global inquiry config merged with the parent assistant.
    pub(crate) fn config_for(&self, tool_name: &str, question_id: &str) -> &InquiryConfig {
        self.overrides
            .get(&(tool_name.to_owned(), question_id.to_owned()))
            .unwrap_or(&self.default_config)
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl InquiryBackend for LlmInquiryBackend {
    async fn inquire(
        &self,
        mut events: ConversationStream,
        inquiry_id: &str,
        tool_name: &str,
        question: &Question,
        cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError> {
        let config = self.config_for(tool_name, question.id.as_str());

        info!(
            inquiry_id,
            tool_name,
            model = config.model.name(),
            question_id = %question.id,
            question_type = ?question.answer_type,
            question_text = %question.text,
            "Structured inquiry initiated",
        );

        // Sanitize the cloned stream to fix structural invariants before
        // sending to the provider. In particular, this adds synthetic
        // ToolCallResponses for any concurrent tool calls that haven't
        // completed yet (their ToolCallRequests are in the stream but
        // the responses aren't).
        events.sanitize();

        // Truncate older events if the inquiry model has a smaller context
        // window than what the conversation has accumulated.
        if let Some(context_window) = config.model.context_window {
            let overhead = window::estimate_overhead_chars(
                config.system_prompt.as_deref(),
                &config.sections,
                &self.attachments,
                &self.tools,
            );

            window::truncate_to_fit(&mut events, context_window, overhead);
        }

        // Tag the second-to-last provider-visible event with a cache
        // breakpoint hint. The last provider-visible event is the synthetic
        // ToolCallResponse ("Tool paused: ...") added by spawn_inquiry,
        // which has a unique tool call ID per inquiry. We tag the event
        // BEFORE it so providers cache the stable prefix and not the
        // per-inquiry tail. Multiple inquiries to the same model within a
        // turn benefit from caching the shared prefix.
        {
            let target_idx = second_last_visible_event_index(&events);
            if let Some(idx) = target_idx
                && let Some(event_ref) = events.iter_mut().nth(idx)
            {
                event_ref
                    .event
                    .add_metadata_field(jp_conversation::event::CACHE_BREAKPOINT_KEY, true);
            }
        }

        // Append the user-facing question with the structured output schema.
        // The caller is responsible for any context events (e.g. a
        // ToolCallResponse) that should precede this in the stream.
        events.start_turn(ChatRequest {
            content: format!(
                "The tool `{tool_name}` requires additional input.\n\n{}\n\nProvide your answer \
                 based on the conversation context.",
                question.text,
            ),
            schema: Some(create_inquiry_schema(question)),
            author: None,
        });

        let thread = Thread {
            system_prompt: config.system_prompt.clone(),
            sections: config.sections.clone(),
            attachments: self.attachments.clone(),
            events,
        };

        let query = ChatQuery {
            thread,
            tools: self.tools.clone(),
            tool_choice: ToolChoice::None,
        };

        let retry_config =
            RetryConfig::default().with_max_response_bytes(config.max_response_bytes);
        let llm_events = tokio::select! {
            biased;
            () = cancellation_token.cancelled() => {
                return Err(InquiryError::Cancelled);
            }
            result = collect_with_retry(
                config.provider.as_ref(),
                &config.model,
                query,
                &retry_config,
            ) => {
                result?
            }
        };

        let mut structured_data =
            structured_data(llm_events).ok_or(InquiryError::MissingStructuredData)?;

        info!(
            inquiry_id,
            answer = %structured_data,
            "Structured inquiry completed",
        );

        // A literal `null` answer is rejected here rather than recorded: it is
        // never a meaningful answer, and `InquiryResponse::answered` refuses
        // null values.
        structured_data
            .get_mut("answer")
            .map(Value::take)
            .filter(|answer| !answer.is_null())
            .ok_or_else(|| {
                let reason = format!(
                    "missing or null 'answer' field in structured response: {}",
                    serde_json::to_string(&structured_data)
                        .unwrap_or_else(|_| "<unparsable>".into())
                );

                tracing::warn!(
                    inquiry_id,
                    raw_data = %structured_data,
                    %reason,
                    "Inquiry answer extraction failed",
                );

                InquiryError::AnswerExtraction { reason }
            })
    }
}

/// Test double that returns pre-configured answers keyed by inquiry ID.
#[cfg(test)]
pub struct MockInquiryBackend {
    answers: std::collections::HashMap<String, Value>,
}

#[cfg(test)]
impl MockInquiryBackend {
    pub fn new(answers: std::collections::HashMap<String, Value>) -> Self {
        Self { answers }
    }
}

#[cfg(test)]
#[async_trait]
impl InquiryBackend for MockInquiryBackend {
    async fn inquire(
        &self,
        _events: ConversationStream,
        inquiry_id: &str,
        _tool_name: &str,
        _question: &Question,
        _cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError> {
        self.answers
            .get(inquiry_id)
            .cloned()
            .ok_or_else(|| InquiryError::Other(format!("No mock answer for inquiry: {inquiry_id}")))
    }
}

/// Find the iteration index of the second-to-last provider-visible event.
///
/// Returns `None` if there are fewer than two visible events.
fn second_last_visible_event_index(events: &ConversationStream) -> Option<usize> {
    let mut last = None;
    let mut second_last = None;

    for (i, event_ref) in events.iter().enumerate() {
        if event_ref.event.kind.is_provider_visible() {
            second_last = last;
            last = Some(i);
        }
    }

    second_last
}

#[cfg(test)]
#[path = "inquiry_tests.rs"]
mod tests;
