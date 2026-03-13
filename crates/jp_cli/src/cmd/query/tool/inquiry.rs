//! Structured inquiry support for tool execution.
//!
//! When a tool requires additional input with `QuestionTarget::Assistant`,
//! the `ToolCoordinator` spawns an async inquiry task that makes a structured
//! output request to the LLM, extracts the answer, and sends it back via the
//! event channel. The tool is then re-executed with the answer.
//!
//! This module provides:
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
use jp_conversation::{
    ConversationEvent, ConversationStream, EventKind,
    event::{ChatRequest, ChatResponse},
    event_builder::EventBuilder,
    thread::Thread,
};
use jp_llm::{
    Provider,
    event::Event,
    model::ModelDetails,
    query::ChatQuery,
    retry::{RetryConfig, collect_with_retry},
    tool::ToolDefinition,
};
use jp_tool::{AnswerType, Question};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Create an inquiry ID for a tool call question.
///
/// Format: `<tool_call_id>.<question_id>` — unique per question within a
/// tool call. The tool name is not included because it's already stored
/// in the `InquirySource` of the `InquiryRequest` event.
pub fn tool_call_inquiry_id(tool_call_id: &str, question_id: &str) -> String {
    format!("{tool_call_id}.{question_id}")
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
        AnswerType::Text => json!({
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
/// call to an LLM provider. Tests use `MockInquiryBackend` to return
/// pre-configured answers without network calls.
#[async_trait]
pub trait InquiryBackend: Send + Sync {
    /// Resolve an inquiry and return the answer.
    ///
    /// `events` is an owned conversation stream (cloned just-in-time by the
    /// caller). The implementation appends temporary events, builds a thread,
    /// makes the LLM call, and extracts the answer.
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
/// Built at turn start from the three-layer merge:
/// per-question `PartialAssistantConfig` → global inquiry config → main assistant config.
pub struct InquiryConfig {
    pub provider: Arc<dyn Provider>,
    pub model: ModelDetails,
    pub system_prompt: Option<String>,
    pub sections: Vec<SectionConfig>,
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
    /// Built at turn start from `QuestionTarget::Assistant(config)` entries
    /// in the active tool configs.
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
    fn config_for(&self, tool_name: &str, question_id: &str) -> &InquiryConfig {
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
        let config = self.config_for(tool_name, &question.id);

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
        if let Some(max_tokens) = config.model.context_window {
            let overhead = estimate_fixed_overhead_chars(
                config.system_prompt.as_deref(),
                &config.sections,
                &self.attachments,
                &self.tools,
            );

            truncate_to_fit(&mut events, max_tokens, overhead);
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

        let retry_config = RetryConfig::default();
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

        // Pipe raw streaming events through the EventBuilder so that
        // structured JSON chunks are concatenated and parsed into a
        // proper Value (rather than individual Value::String fragments).
        let mut builder = EventBuilder::new();
        let mut flushed = Vec::new();
        for event in llm_events {
            match event {
                Event::Part { index, event } => builder.handle_part(index, event),
                Event::Flush { index, metadata } => {
                    flushed.extend(builder.handle_flush(index, metadata));
                }
                Event::Finished(_) => flushed.extend(builder.drain()),
            }
        }

        let mut structured_data = flushed
            .into_iter()
            .filter_map(ConversationEvent::into_chat_response)
            .find_map(ChatResponse::into_structured_data)
            .ok_or(InquiryError::MissingStructuredData)?;

        info!(
            inquiry_id,
            answer = %structured_data,
            "Structured inquiry completed",
        );

        structured_data
            .get_mut("answer")
            .map(Value::take)
            .ok_or_else(|| {
                let reason = format!(
                    "missing 'answer' field in structured response: {}",
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

/// Estimated chars-per-token ratio used for estimation.
const CHARS_PER_TOKEN: usize = 3;

/// Safety margin for tokenization imprecision (the 3 chars/token ratio varies
/// by content type) and provider framing overhead (JSON wrapping, role tags,
/// structured output injection, etc.).
///
/// The system prompt, sections, attachments, and tool definitions are now
/// measured explicitly via `estimate_fixed_overhead_chars`, so this factor
/// only needs to cover the remaining approximation error.
const OVERHEAD_FACTOR: usize = 90; // percent

/// When truncation is needed, target this fraction of the context window.
/// Leaves headroom so subsequent inquiries in the same turn don't re-truncate
/// (which would bust the prompt cache).
const TARGET_FACTOR: usize = 80; // percent

fn estimate_chars(events: &ConversationStream) -> usize {
    events.iter().map(|e| estimate_event_chars(e.event)).sum()
}

fn token_budget(max_tokens: u32, overhead_chars: usize) -> usize {
    let total = (max_tokens as usize) * CHARS_PER_TOKEN * OVERHEAD_FACTOR / 100;
    total.saturating_sub(overhead_chars)
}

fn token_target(max_tokens: u32, overhead_chars: usize) -> usize {
    let total = (max_tokens as usize) * CHARS_PER_TOKEN * TARGET_FACTOR / 100;
    total.saturating_sub(overhead_chars)
}

fn estimate_event_chars(event: &ConversationEvent) -> usize {
    match &event.kind {
        EventKind::ChatRequest(r) => r.content.len(),
        EventKind::ChatResponse(ChatResponse::Message { message }) => message.len(),
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => reasoning.len(),
        EventKind::ChatResponse(ChatResponse::Structured { data }) => data.to_string().len(),
        EventKind::ToolCallRequest(r) => {
            r.name.len() + serde_json::to_string(&r.arguments).map_or(0, |s| s.len())
        }
        EventKind::ToolCallResponse(r) => {
            r.result.as_ref().map_or(0, String::len)
                + r.result.as_ref().err().map_or(0, String::len)
        }
        _ => 0,
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

/// Char-based estimate of the fixed overhead that shares the model's context
/// window with conversation events: system prompt, sections, attachments, and
/// tool definitions.
fn estimate_fixed_overhead_chars(
    system_prompt: Option<&str>,
    sections: &[SectionConfig],
    attachments: &[Attachment],
    tools: &[ToolDefinition],
) -> usize {
    let mut chars = 0;

    if let Some(prompt) = system_prompt {
        chars += prompt.len();
    }

    for section in sections {
        chars += section.render().len();
    }

    for attachment in attachments {
        if let Some(text) = attachment.as_text() {
            chars += text.len();
        }
        // Binary attachments also consume tokens (base64, etc.) but we can't
        // easily measure them. The OVERHEAD_FACTOR margin covers this.
    }

    for tool in tools {
        chars += tool.name.len();
        if let Some(desc) = tool.docs.schema_description() {
            chars += desc.len();
        }
        // Parameter schemas are serialized as JSON by providers.
        chars += serde_json::to_string(&tool.to_parameters_schema()).map_or(0, |s| s.len());
    }

    chars
}

/// Drop older events so the conversation fits within the model's context
/// window.
///
/// The budget is calculated from the model's context window minus the measured
/// fixed overhead (system prompt, sections, attachments, tool definitions).
/// When the estimated char count of conversation events exceeds this budget,
/// events are dropped from the **start** until enough chars have been removed
/// to bring the total under the target.
///
/// Dropping from the start (oldest events) produces a stable cutoff across
/// multiple inquiry calls within the same turn. Since `ConversationStream`
/// is append-only, the same K oldest events are dropped regardless of how
/// many new events were appended at the end. This preserves the prompt
/// cache prefix across inquiries.
///
/// After truncation, `sanitize()` restores structural invariants
/// (orphaned tool calls, leading non-user events, etc.).
fn truncate_to_fit(events: &mut ConversationStream, max_tokens: u32, overhead_chars: usize) {
    let budget = token_budget(max_tokens, overhead_chars);
    let total_chars = estimate_chars(events);

    if total_chars <= budget {
        return;
    }

    let target = token_target(max_tokens, overhead_chars);

    // Round must_drop up to the nearest 10% of target so that small
    // additions at the end of the stream (e.g. a tool response from a
    // previous inquiry) don't shift the cutoff point. This keeps the
    // prefix stable across multiple inquiry calls in the same turn,
    // preserving prompt cache hits on the conversation messages.
    let granularity = target / 10;
    let raw_drop = total_chars.saturating_sub(target);
    let must_drop = match granularity {
        0 => raw_drop,
        g => raw_drop.div_ceil(g) * g,
    };

    // Walk from the start (oldest), accumulating chars to drop.
    let char_counts: Vec<usize> = events
        .iter()
        .map(|e| estimate_event_chars(e.event))
        .collect();

    let mut dropped_chars = 0;
    let mut dropped_events = 0;

    for count in &char_counts {
        if dropped_chars >= must_drop {
            break;
        }
        dropped_chars += count;
        dropped_events += 1;
    }

    let mut idx = 0;
    events.retain(|_| {
        let keep = idx >= dropped_events;
        idx += 1;
        keep
    });

    events.sanitize();

    // If truncation removed all ChatRequests, the remaining events (assistant
    // responses, tool calls) cannot form a valid provider message sequence —
    // providers require the first message to be a user message. Clear the
    // stream so the inquiry's own ChatRequest (added by the caller via
    // `start_turn`) becomes the only content.
    if !events.has_chat_request() {
        events.clear();
    }

    info!(
        max_tokens,
        dropped_events, "Truncated inquiry context to fit model window",
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use jp_config::model::id::{ModelIdConfig, ProviderId};
    use jp_conversation::{
        ConversationStream,
        event::{
            ConversationEvent, InquiryQuestion, InquiryRequest, InquirySource, ToolCallRequest,
            ToolCallResponse,
        },
    };
    use jp_llm::{
        event::{Event, FinishReason},
        provider::mock::MockProvider,
        tool::ToolDocs,
    };

    use super::*;

    /// Build a `MockProvider` that returns a structured JSON response.
    ///
    /// Emits the data as a `Value::String` chunk (matching how real providers
    /// stream structured output) so the `EventBuilder` can parse it on flush.
    #[expect(clippy::needless_pass_by_value)]
    fn structured_provider(data: Value) -> MockProvider {
        MockProvider::new(vec![
            Event::Part {
                index: 0,
                event: ConversationEvent::now(ChatResponse::structured(Value::String(
                    data.to_string(),
                ))),
            },
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ])
    }

    fn test_model() -> ModelDetails {
        ModelDetails::empty(ModelIdConfig {
            provider: ProviderId::Test,
            name: "mock".parse().unwrap(),
        })
    }

    fn test_inquiry_config(provider: MockProvider) -> InquiryConfig {
        InquiryConfig {
            provider: Arc::new(provider),
            model: test_model(),
            system_prompt: None,
            sections: vec![],
        }
    }

    fn test_question() -> Question {
        Question {
            id: "confirm".to_string(),
            text: "Create backup?".to_string(),
            answer_type: AnswerType::Boolean,
            default: None,
        }
    }

    fn test_events() -> ConversationStream {
        ConversationStream::new_test().with_turn("Modify file X")
    }

    #[test]
    fn test_tool_call_inquiry_id() {
        assert_eq!(
            tool_call_inquiry_id("call_abc123", "apply_changes"),
            "call_abc123.apply_changes"
        );
    }

    #[test]
    fn test_tool_call_inquiry_id_unique_per_question() {
        let id_a = tool_call_inquiry_id("call_1", "confirm");
        let id_b = tool_call_inquiry_id("call_1", "reason");
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_create_inquiry_schema_boolean() {
        let question = Question {
            id: "q1".to_string(),
            text: "Confirm?".to_string(),
            answer_type: AnswerType::Boolean,
            default: None,
        };

        let schema = create_inquiry_schema(&question);

        assert_eq!(schema.get("type"), Some(&json!("object")));

        let props = schema.get("properties").and_then(Value::as_object).unwrap();
        assert_eq!(
            props.get("answer"),
            Some(&json!({
                "type": "boolean"
            }))
        );

        assert_eq!(schema.get("required"), Some(&json!(["answer"])));
        assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
    }

    #[test]
    fn test_create_inquiry_schema_select() {
        let question = Question {
            id: "q2".to_string(),
            text: "Choose one".to_string(),
            answer_type: AnswerType::Select {
                options: vec!["A".to_string(), "B".to_string(), "C".to_string()],
            },
            default: None,
        };

        let schema = create_inquiry_schema(&question);
        let props = schema.get("properties").and_then(Value::as_object).unwrap();

        assert_eq!(
            props.get("answer"),
            Some(&json!({
                "type": "string",
                "enum": ["A", "B", "C"]
            }))
        );
    }

    #[test]
    fn test_create_inquiry_schema_text() {
        let question = Question {
            id: "q3".to_string(),
            text: "Enter text".to_string(),
            answer_type: AnswerType::Text,
            default: None,
        };

        let schema = create_inquiry_schema(&question);
        let props = schema.get("properties").and_then(Value::as_object).unwrap();

        assert_eq!(
            props.get("answer"),
            Some(&json!({
                "type": "string"
            }))
        );
    }

    #[test]
    fn test_create_inquiry_schema_stable_across_ids() {
        let question = Question {
            id: "q1".to_string(),
            text: "Confirm?".to_string(),
            answer_type: AnswerType::Boolean,
            default: None,
        };

        let schema_a = create_inquiry_schema(&question);
        let schema_b = create_inquiry_schema(&question);
        assert_eq!(schema_a, schema_b);
    }

    #[tokio::test]
    async fn llm_backend_returns_answer() {
        let inquiry_id = tool_call_inquiry_id("call_abc", "confirm");
        let config = InquiryConfig {
            system_prompt: Some("You are a helpful assistant.".to_string()),
            ..test_inquiry_config(structured_provider(json!({ "answer": true })))
        };

        let backend = LlmInquiryBackend::new(config, IndexMap::new(), vec![], vec![]);

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), json!(true));
    }

    #[tokio::test]
    async fn llm_backend_returns_error_on_missing_structured_data() {
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");
        let config = test_inquiry_config(MockProvider::with_message("I don't know"));
        let backend = LlmInquiryBackend::new(config, IndexMap::new(), vec![], vec![]);

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert!(matches!(result, Err(InquiryError::MissingStructuredData)));
    }

    #[tokio::test]
    async fn llm_backend_returns_error_on_answer_extraction_failure() {
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");
        let config = test_inquiry_config(structured_provider(json!({ "unrelated": true })));
        let backend = LlmInquiryBackend::new(config, IndexMap::new(), vec![], vec![]);

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert!(matches!(result, Err(InquiryError::AnswerExtraction { .. })));
    }

    #[tokio::test]
    async fn llm_backend_returns_cancelled_when_token_is_already_cancelled() {
        let config = test_inquiry_config(structured_provider(json!({ "answer": true })));
        let backend = LlmInquiryBackend::new(config, IndexMap::new(), vec![], vec![]);
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");

        let token = CancellationToken::new();
        token.cancel();

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &test_question(),
                token,
            )
            .await;

        assert!(matches!(result, Err(InquiryError::Cancelled)));
    }

    #[tokio::test]
    async fn llm_backend_passes_select_question() {
        let inquiry_id = tool_call_inquiry_id("call_sel", "choose");
        let question = Question {
            id: "choose".to_string(),
            text: "Pick one".to_string(),
            answer_type: AnswerType::Select {
                options: vec!["A".to_string(), "B".to_string()],
            },
            default: None,
        };
        let config = test_inquiry_config(structured_provider(json!({ "answer": "B" })));
        let backend = LlmInquiryBackend::new(config, IndexMap::new(), vec![], vec![]);

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &question,
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), json!("B"));
    }

    #[tokio::test]
    async fn llm_backend_passes_text_question() {
        let inquiry_id = tool_call_inquiry_id("call_txt", "reason");
        let question = Question {
            id: "reason".to_string(),
            text: "Why?".to_string(),
            answer_type: AnswerType::Text,
            default: None,
        };
        let config =
            test_inquiry_config(structured_provider(json!({ "answer": "Because reasons" })));
        let backend = LlmInquiryBackend::new(config, IndexMap::new(), vec![], vec![]);

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &question,
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), json!("Because reasons"));
    }

    #[tokio::test]
    async fn mock_backend_returns_configured_answer() {
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");
        let backend = MockInquiryBackend::new(HashMap::from([(inquiry_id.clone(), json!(true))]));

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), json!(true));
    }

    #[tokio::test]
    async fn mock_backend_returns_error_for_unknown_inquiry() {
        let backend = MockInquiryBackend::new(HashMap::new());

        let result = backend
            .inquire(
                test_events(),
                "tool_call.unknown.call_999",
                "test_tool",
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert!(matches!(result, Err(InquiryError::Other(_))));
    }

    #[tokio::test]
    async fn mock_backend_ignores_cancellation_token() {
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");
        let backend = MockInquiryBackend::new(HashMap::from([(inquiry_id.clone(), json!(42))]));

        // Even with a cancelled token, mock returns immediately.
        let token = CancellationToken::new();
        token.cancel();

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &test_question(),
                token,
            )
            .await;

        assert_eq!(result.unwrap(), json!(42));
    }

    #[tokio::test]
    async fn llm_backend_uses_per_question_override() {
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");
        let default_config = test_inquiry_config(
            // Default provider returns wrong data (would fail extraction).
            structured_provider(json!({ "unrelated": true })),
        );

        let override_config = InquiryConfig {
            provider: Arc::new(structured_provider(json!({ "answer": true }))),
            model: test_model(),
            system_prompt: Some("Override prompt.".into()),
            sections: vec![],
        };

        let overrides = IndexMap::from([(("test_tool".into(), "confirm".into()), override_config)]);

        let backend = LlmInquiryBackend::new(default_config, overrides, vec![], vec![]);

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), json!(true));
    }

    // -- second_last_visible_event_index tests --

    #[test]
    fn visible_index_empty_stream() {
        let events = ConversationStream::new_test();
        assert_eq!(second_last_visible_event_index(&events), None);
    }

    #[test]
    fn visible_index_single_turn() {
        // A single turn has [TurnStart, ChatRequest] — only ChatRequest is visible.
        let events = ConversationStream::new_test().with_turn("hello");
        assert_eq!(second_last_visible_event_index(&events), None);
    }

    #[test]
    fn visible_index_two_visible_events() {
        // [TurnStart, ChatRequest, ChatResponse] — 2 visible events.
        let mut events = ConversationStream::new_test();
        events.start_turn("hello");
        events
            .current_turn_mut()
            .add_chat_response(ChatResponse::message("world"))
            .build()
            .unwrap();

        let idx = second_last_visible_event_index(&events).unwrap();
        let event_at_idx = events.iter().nth(idx).unwrap();
        assert!(matches!(event_at_idx.event.kind, EventKind::ChatRequest(_)));
    }

    /// Reproduces the bug: when an `InquiryRequest` sits between the last
    /// `ToolCallRequest` and the synthetic `ToolCallResponse`, the old code
    /// would place the breakpoint on the non-visible `InquiryRequest`.
    #[test]
    fn visible_index_skips_inquiry_request() {
        let mut events = ConversationStream::new_test();
        events.start_turn("do something");
        events
            .current_turn_mut()
            .add_chat_response(ChatResponse::message("I'll call a tool."))
            .add_tool_call_request(ToolCallRequest {
                id: "call_1".into(),
                name: "test_tool".into(),
                arguments: Map::default(),
            })
            .add_inquiry_request(InquiryRequest::new(
                "call_1.confirm",
                InquirySource::tool("test_tool"),
                InquiryQuestion::boolean("Proceed?".into()),
            ))
            .add_tool_call_response(ToolCallResponse {
                id: "call_1".into(),
                result: Ok("Tool paused: Proceed?".into()),
            })
            .build()
            .unwrap();

        let idx = second_last_visible_event_index(&events).unwrap();
        let event_at_idx = events.iter().nth(idx).unwrap();

        // Must be the ToolCallRequest, NOT the InquiryRequest.
        assert!(matches!(
            event_at_idx.event.kind,
            EventKind::ToolCallRequest(_)
        ));
    }

    // -- estimate_fixed_overhead_chars tests --

    #[test]
    fn overhead_empty_inputs() {
        assert_eq!(estimate_fixed_overhead_chars(None, &[], &[], &[]), 0);
    }

    #[test]
    fn overhead_system_prompt() {
        let prompt = "You are a helpful assistant.";
        let result = estimate_fixed_overhead_chars(Some(prompt), &[], &[], &[]);
        assert_eq!(result, prompt.len());
    }

    #[test]
    fn overhead_sections() {
        let section = SectionConfig::default()
            .with_tag("instruction")
            .with_title("Testing")
            .with_content("Do the thing.");
        let rendered_len = section.render().len();

        let result = estimate_fixed_overhead_chars(None, &[section], &[], &[]);
        assert_eq!(result, rendered_len);
    }

    #[test]
    fn overhead_text_attachments() {
        let attachment = Attachment::text("file.rs", "fn main() {}");
        let result = estimate_fixed_overhead_chars(None, &[], &[attachment], &[]);
        assert_eq!(result, "fn main() {}".len());
    }

    #[test]
    fn overhead_binary_attachments_ignored() {
        let attachment = Attachment::binary("img.png", vec![0u8; 1000], "image/png");
        let result = estimate_fixed_overhead_chars(None, &[], &[attachment], &[]);
        assert_eq!(result, 0);
    }

    #[test]
    fn overhead_tool_definitions() {
        let tool = ToolDefinition {
            name: "grep_files".to_string(),
            docs: ToolDocs {
                summary: Some("Search files.".to_string()),
                ..Default::default()
            },
            parameters: IndexMap::new(),
        };
        let result = estimate_fixed_overhead_chars(None, &[], &[], &[tool]);
        // name + description + serialized schema
        assert!(result > 0);
        assert!(result > "grep_files".len() + "Search files.".len());
    }

    #[test]
    fn overhead_combines_all_sources() {
        let prompt = "Be helpful.";
        let section = SectionConfig::default().with_content("Rule 1.");
        let attachment = Attachment::text("f.txt", "hello world");
        let tool = ToolDefinition {
            name: "t".to_string(),
            docs: ToolDocs::default(),
            parameters: IndexMap::new(),
        };

        let combined = estimate_fixed_overhead_chars(
            Some(prompt),
            std::slice::from_ref(&section),
            std::slice::from_ref(&attachment),
            std::slice::from_ref(&tool),
        );

        let sum = estimate_fixed_overhead_chars(Some(prompt), &[], &[], &[])
            + estimate_fixed_overhead_chars(None, std::slice::from_ref(&section), &[], &[])
            + estimate_fixed_overhead_chars(None, &[], std::slice::from_ref(&attachment), &[])
            + estimate_fixed_overhead_chars(None, &[], &[], std::slice::from_ref(&tool));

        assert_eq!(combined, sum);
    }

    // -- token_budget / token_target with overhead --

    #[test]
    fn budget_subtracts_overhead() {
        let no_overhead = token_budget(1000, 0);
        let with_overhead = token_budget(1000, 500);
        assert_eq!(no_overhead - 500, with_overhead);
    }

    #[test]
    fn budget_saturates_at_zero() {
        // Overhead larger than total budget shouldn't underflow.
        assert_eq!(token_budget(100, 999_999), 0);
    }

    #[test]
    fn target_subtracts_overhead() {
        let no_overhead = token_target(1000, 0);
        let with_overhead = token_target(1000, 500);
        assert_eq!(no_overhead - 500, with_overhead);
    }

    // -- truncate_to_fit with overhead --

    #[test]
    fn truncate_no_op_when_within_budget() {
        let mut events = ConversationStream::new_test().with_turn("short");
        let count_before = events.len();
        // Large context window, no overhead => no truncation.
        truncate_to_fit(&mut events, 100_000, 0);
        assert_eq!(events.len(), count_before);
    }

    #[test]
    fn truncate_triggers_with_overhead() {
        // Build a stream that fits in the raw budget but not after subtracting
        // overhead. Each turn adds ~20 chars ("message N" is ~9 chars for
        // request + response).
        let mut events = ConversationStream::new_test();
        for i in 0..50 {
            events = events.with_turn(format!("message {i} with some padding text here"));
        }

        let total_chars = estimate_chars(&events);
        let count_before = events.len();

        // Pick a context window where total_chars fits at 90% but not after
        // subtracting a large overhead.
        #[expect(clippy::cast_possible_truncation)]
        let max_tokens = ((total_chars * 100) / (CHARS_PER_TOKEN * OVERHEAD_FACTOR) + 100) as u32;

        // Without overhead, no truncation.
        let mut no_overhead = events.clone();
        truncate_to_fit(&mut no_overhead, max_tokens, 0);
        assert_eq!(no_overhead.len(), count_before);

        // With overhead eating most of the budget, truncation should happen.
        let overhead = token_budget(max_tokens, 0) - 100;
        truncate_to_fit(&mut events, max_tokens, overhead);
        assert!(events.len() < count_before);
    }

    #[tokio::test]
    async fn dedicated_model_backend_returns_answer() {
        let inquiry_id = tool_call_inquiry_id("call_dedicated", "confirm");
        let config = InquiryConfig {
            provider: Arc::new(structured_provider(json!({ "answer": true }))),
            model: ModelDetails::empty(ModelIdConfig {
                provider: ProviderId::Test,
                name: "cheap-model".parse().unwrap(),
            }),
            system_prompt: Some("Answer concisely.".to_string()),
            sections: vec![],
        };

        let backend = LlmInquiryBackend::new(config, IndexMap::new(), vec![], vec![]);

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                "test_tool",
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), json!(true));
    }
}
