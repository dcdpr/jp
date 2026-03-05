//! Structured inquiry support for tool execution.
//!
//! When a tool requires additional input with `QuestionTarget::Assistant`,
//! the `ToolCoordinator` spawns an async inquiry task that makes a structured
//! output request to the LLM, extracts the answer, and sends it back via the
//! event channel. The tool is then re-executed with the answer.
//!
//! This module provides:
//! - Schema generation and answer extraction ([`ActiveInquiry`])
//! - The [`InquiryBackend`] trait for testability
//! - [`LlmInquiryBackend`] for real LLM calls
//! - `MockInquiryBackend` for tests
//!
//! See `docs/architecture/stateful-tool-inquiries.md` for the full design.

use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use jp_attachment::Attachment;
use jp_config::assistant::{sections::SectionConfig, tool_choice::ToolChoice};
use jp_conversation::{
    ConversationEvent, ConversationStream,
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
};
use jp_tool::{AnswerType, Question};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

/// Helper for constructing and validating a structured inquiry.
///
/// Created on the stack during the inquiry cycle. The `TurnCoordinator`
/// tracks inquiry state (phase, stream index); this struct handles only
/// schema generation and answer extraction.
#[derive(Debug)]
pub struct ActiveInquiry {
    /// Opaque identifier for this inquiry.
    ///
    /// Format depends on the inquiry source:
    /// - Tool calls: `tool_call.<tool_name>.<tool_call_id>`
    /// - Future sources: `<source_type>.<context_id>`
    pub id: String,

    /// The JSON schema used for structured output.
    pub schema: Map<String, Value>,

    /// When this inquiry was created (for duration logging).
    pub created_at: Instant,
}

impl ActiveInquiry {
    /// Create a new inquiry from an opaque ID and question.
    ///
    /// The schema is pre-computed from the ID and question type.
    #[must_use]
    pub fn new(id: String, question: &Question) -> Self {
        let schema = create_inquiry_schema(&id, question);

        Self {
            id,
            schema,
            created_at: Instant::now(),
        }
    }

    /// Extract the answer from a structured response.
    ///
    /// Validates that the `inquiry_id` matches (if present) and returns
    /// the `answer` field. Returns a descriptive error on failure.
    pub fn extract_answer(&self, response: &Value) -> Result<Value, String> {
        if let Some(response_id) = response.get("inquiry_id").and_then(Value::as_str)
            && response_id != self.id
        {
            return Err(format!(
                "inquiry_id mismatch: expected '{}', got '{}'",
                self.id, response_id
            ));
        }

        response.get("answer").cloned().ok_or_else(|| {
            format!(
                "missing 'answer' field in structured response: {}",
                serde_json::to_string(response).unwrap_or_else(|_| "<unparseable>".into())
            )
        })
    }

    /// Get elapsed time since inquiry creation.
    #[must_use]
    pub fn elapsed(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }
}

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
/// The schema enforces:
/// 1. `inquiry_id` field with a `const` value (may be rewritten by provider)
/// 2. `answer` field with type matching the question's `answer_type`
///
/// Providers that don't support `const` will rewrite it to `enum` with a
/// single value or fall back to a description hint. See the schema
/// compatibility section in `docs/architecture/stateful-tool-inquiries.md`.
pub fn create_inquiry_schema(inquiry_id: &str, question: &Question) -> Map<String, Value> {
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
                "inquiry_id": {
                    "type": "string",
                    "const": inquiry_id
                },
                "answer": answer_schema
            }),
        ),
        ("required".into(), json!(["inquiry_id", "answer"])),
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
    /// `question` describes the expected answer type and text.
    async fn inquire(
        &self,
        events: ConversationStream,
        inquiry_id: &str,
        question: &Question,
        cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError>;
}

/// Resolves inquiries by making structured output calls to an LLM provider.
///
/// Holds the static parts of a conversation thread (system prompt, sections,
/// attachments) that stay constant for the duration of a turn. The dynamic
/// conversation events are passed in per-call via the `events` parameter.
pub struct LlmInquiryBackend {
    provider: Arc<dyn Provider>,
    model: ModelDetails,
    system_prompt: Option<String>,
    sections: Vec<SectionConfig>,
    attachments: Vec<Attachment>,
}

impl LlmInquiryBackend {
    /// Create a new LLM-backed inquiry backend.
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        model: ModelDetails,
        system_prompt: Option<String>,
        sections: Vec<SectionConfig>,
        attachments: Vec<Attachment>,
    ) -> Self {
        Self {
            provider,
            model,
            system_prompt,
            sections,
            attachments,
        }
    }
}

#[async_trait]
impl InquiryBackend for LlmInquiryBackend {
    async fn inquire(
        &self,
        mut events: ConversationStream,
        inquiry_id: &str,
        question: &Question,
        cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError> {
        let inquiry = ActiveInquiry::new(inquiry_id.to_string(), question);

        tracing::info!(
            inquiry_id,
            question_id = %question.id,
            question_type = ?question.answer_type,
            question_text = %question.text,
            "Structured inquiry initiated",
        );

        // Append the user-facing question with the structured output schema.
        // The caller is responsible for any context events (e.g. a
        // ToolCallResponse) that should precede this in the stream.
        events.start_turn(ChatRequest {
            content: format!(
                "A tool requires additional input.\n\n{}\n\nProvide your answer based on the \
                 conversation context.",
                question.text,
            ),
            schema: Some(inquiry.schema.clone()),
        });

        let thread = Thread {
            system_prompt: self.system_prompt.clone(),
            sections: self.sections.clone(),
            attachments: self.attachments.clone(),
            events,
        };

        let query = ChatQuery {
            thread,
            tools: vec![],
            tool_choice: ToolChoice::None,
        };

        let retry_config = RetryConfig::default();
        let llm_events = tokio::select! {
            biased;
            () = cancellation_token.cancelled() => {
                return Err(InquiryError::Cancelled);
            }
            result = collect_with_retry(
                self.provider.as_ref(),
                &self.model,
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

        let structured_data = flushed
            .into_iter()
            .filter_map(ConversationEvent::into_chat_response)
            .find_map(ChatResponse::into_structured_data)
            .ok_or(InquiryError::MissingStructuredData)?;

        tracing::info!(
            inquiry_id,
            answer = %structured_data,
            elapsed_ms = %inquiry.elapsed().as_millis(),
            "Structured inquiry completed",
        );

        inquiry.extract_answer(&structured_data).map_err(|reason| {
            tracing::warn!(
                inquiry_id,
                %reason,
                raw_data = %structured_data,
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
        _question: &Question,
        _cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError> {
        self.answers
            .get(inquiry_id)
            .cloned()
            .ok_or_else(|| InquiryError::Other(format!("No mock answer for inquiry: {inquiry_id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let schema = create_inquiry_schema("tool_call.my_tool.call_123", &question);

        assert_eq!(schema.get("type"), Some(&json!("object")));

        let props = schema.get("properties").and_then(Value::as_object).unwrap();
        assert_eq!(
            props.get("inquiry_id"),
            Some(&json!({
                "type": "string",
                "const": "tool_call.my_tool.call_123"
            }))
        );
        assert_eq!(
            props.get("answer"),
            Some(&json!({
                "type": "boolean"
            }))
        );

        assert_eq!(
            schema.get("required"),
            Some(&json!(["inquiry_id", "answer"]))
        );
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

        let schema = create_inquiry_schema("tool_call.my_tool.call_456", &question);
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

        let schema = create_inquiry_schema("tool_call.my_tool.call_789", &question);
        let props = schema.get("properties").and_then(Value::as_object).unwrap();

        assert_eq!(
            props.get("answer"),
            Some(&json!({
                "type": "string"
            }))
        );
    }

    #[test]
    fn test_extract_answer_valid() {
        let question = Question {
            id: "q1".to_string(),
            text: "Test?".to_string(),
            answer_type: AnswerType::Boolean,
            default: None,
        };

        let inquiry = ActiveInquiry::new(tool_call_inquiry_id("call_123", "q1"), &question);

        let response = json!({
            "inquiry_id": "call_123.q1",
            "answer": true
        });

        assert_eq!(inquiry.extract_answer(&response), Ok(json!(true)));
    }

    #[test]
    fn test_extract_answer_without_inquiry_id_still_works() {
        let question = Question {
            id: "q1".to_string(),
            text: "Test?".to_string(),
            answer_type: AnswerType::Boolean,
            default: None,
        };

        let inquiry = ActiveInquiry::new(tool_call_inquiry_id("call_123", "q1"), &question);

        let response = json!({ "answer": true });
        assert_eq!(inquiry.extract_answer(&response), Ok(json!(true)));
    }

    #[test]
    fn test_extract_answer_id_mismatch() {
        let question = Question {
            id: "q1".to_string(),
            text: "Test?".to_string(),
            answer_type: AnswerType::Boolean,
            default: None,
        };

        let inquiry = ActiveInquiry::new(tool_call_inquiry_id("call_123", "q1"), &question);

        let response = json!({
            "inquiry_id": "call_999.q1",
            "answer": true
        });

        let err = inquiry.extract_answer(&response).unwrap_err();
        assert!(err.contains("inquiry_id mismatch"), "got: {err}");
    }

    #[test]
    fn test_extract_answer_missing_answer_field() {
        let question = Question {
            id: "q1".to_string(),
            text: "Test?".to_string(),
            answer_type: AnswerType::Boolean,
            default: None,
        };

        let inquiry = ActiveInquiry::new(tool_call_inquiry_id("call_123", "q1"), &question);

        let response = json!({
            "inquiry_id": "call_123.q1"
        });

        let err = inquiry.extract_answer(&response).unwrap_err();
        assert!(err.contains("missing 'answer' field"), "got: {err}");
    }

    use std::collections::HashMap;

    use jp_conversation::{ConversationStream, event::ConversationEvent};
    use jp_llm::{
        event::{Event, FinishReason},
        provider::mock::MockProvider,
    };

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

    #[tokio::test]
    async fn llm_backend_returns_answer() {
        let inquiry_id = tool_call_inquiry_id("call_abc", "confirm");
        let provider = structured_provider(json!({
            "inquiry_id": inquiry_id,
            "answer": true
        }));

        let backend = LlmInquiryBackend::new(
            Arc::new(provider),
            ModelDetails::empty(jp_config::model::id::ModelIdConfig {
                provider: jp_config::model::id::ProviderId::Test,
                name: "mock".parse().unwrap(),
            }),
            Some("You are a helpful assistant.".to_string()),
            vec![],
            vec![],
        );

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.unwrap(), json!(true));
    }

    #[tokio::test]
    async fn llm_backend_returns_error_on_missing_structured_data() {
        // Provider returns a plain message instead of structured data.
        let provider = MockProvider::with_message("I don't know");
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");

        let backend = LlmInquiryBackend::new(
            Arc::new(provider),
            ModelDetails::empty(jp_config::model::id::ModelIdConfig {
                provider: jp_config::model::id::ProviderId::Test,
                name: "mock".parse().unwrap(),
            }),
            None,
            vec![],
            vec![],
        );

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert!(matches!(result, Err(InquiryError::MissingStructuredData)));
    }

    #[tokio::test]
    async fn llm_backend_returns_error_on_answer_extraction_failure() {
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");
        // Structured data has a mismatched inquiry_id.
        let provider = structured_provider(json!({
            "inquiry_id": "wrong_id",
            "answer": true
        }));

        let backend = LlmInquiryBackend::new(
            Arc::new(provider),
            ModelDetails::empty(jp_config::model::id::ModelIdConfig {
                provider: jp_config::model::id::ProviderId::Test,
                name: "mock".parse().unwrap(),
            }),
            None,
            vec![],
            vec![],
        );

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
                &test_question(),
                CancellationToken::new(),
            )
            .await;

        assert!(matches!(result, Err(InquiryError::AnswerExtraction { .. })));
    }

    #[tokio::test]
    async fn llm_backend_returns_cancelled_when_token_is_already_cancelled() {
        let provider = structured_provider(json!({
            "inquiry_id": "irrelevant",
            "answer": true
        }));
        let inquiry_id = tool_call_inquiry_id("call_1", "confirm");

        let backend = LlmInquiryBackend::new(
            Arc::new(provider),
            ModelDetails::empty(jp_config::model::id::ModelIdConfig {
                provider: jp_config::model::id::ProviderId::Test,
                name: "mock".parse().unwrap(),
            }),
            None,
            vec![],
            vec![],
        );

        let token = CancellationToken::new();
        token.cancel();

        let result = backend
            .inquire(test_events(), &inquiry_id, &test_question(), token)
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
        let provider = structured_provider(json!({
            "inquiry_id": inquiry_id,
            "answer": "B"
        }));

        let backend = LlmInquiryBackend::new(
            Arc::new(provider),
            ModelDetails::empty(jp_config::model::id::ModelIdConfig {
                provider: jp_config::model::id::ProviderId::Test,
                name: "mock".parse().unwrap(),
            }),
            None,
            vec![],
            vec![],
        );

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
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
        let provider = structured_provider(json!({
            "inquiry_id": inquiry_id,
            "answer": "Because reasons"
        }));

        let backend = LlmInquiryBackend::new(
            Arc::new(provider),
            ModelDetails::empty(jp_config::model::id::ModelIdConfig {
                provider: jp_config::model::id::ProviderId::Test,
                name: "mock".parse().unwrap(),
            }),
            None,
            vec![],
            vec![],
        );

        let result = backend
            .inquire(
                test_events(),
                &inquiry_id,
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
            .inquire(test_events(), &inquiry_id, &test_question(), token)
            .await;

        assert_eq!(result.unwrap(), json!(42));
    }
}
