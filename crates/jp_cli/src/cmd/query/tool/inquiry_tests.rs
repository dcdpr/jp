use std::collections::HashMap;

use jp_config::model::id::{ModelIdConfig, ProviderId};
use jp_conversation::{
    ConversationStream, EventKind,
    event::{
        ChatResponse, InquiryQuestion, InquiryRequest, InquirySource, ToolCallRequest,
        ToolCallResponse,
    },
};
use jp_llm::{
    event::{Event, FinishReason},
    provider::mock::MockProvider,
};

use super::*;

/// Build a `MockProvider` that returns a structured JSON response.
///
/// Emits the data as a `Value::String` chunk (matching how real providers
/// stream structured output) so the `EventBuilder` can parse it on flush.
#[expect(clippy::needless_pass_by_value)]
fn structured_provider(data: Value) -> MockProvider {
    MockProvider::new(vec![
        Event::structured(0, data.to_string()),
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
        max_response_bytes: Some(1_048_576),
        assistant: PartialAssistantConfig::default(),
    }
}

fn test_question() -> Question {
    Question::boolean("confirm", "Create backup?").unwrap()
}

fn test_events() -> ConversationStream {
    ConversationStream::new_test().with_turn("Modify file X")
}

#[test]
fn test_tool_call_inquiry_id() {
    assert_eq!(
        tool_call_inquiry_id("call_abc123", "apply_changes", 1),
        "call_abc123.apply_changes.1"
    );
}

#[test]
fn test_tool_call_inquiry_id_unique_per_question() {
    let id_a = tool_call_inquiry_id("call_1", "confirm", 1);
    let id_b = tool_call_inquiry_id("call_1", "reason", 1);
    assert_ne!(id_a, id_b);
}

#[test]
fn test_create_inquiry_schema_boolean() {
    let question = Question::boolean("q1", "Confirm?").unwrap();

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
    let question = Question::select("q2", "Choose one")
        .unwrap()
        .with_options(vec!["A".to_string(), "B".to_string(), "C".to_string()]);

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
    let question = Question::text("q3", "Enter text").unwrap();

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
    let question = Question::boolean("q1", "Confirm?").unwrap();

    let schema_a = create_inquiry_schema(&question);
    let schema_b = create_inquiry_schema(&question);
    assert_eq!(schema_a, schema_b);
}

#[tokio::test]
async fn llm_backend_returns_answer() {
    let inquiry_id = tool_call_inquiry_id("call_abc", "confirm", 1);
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

/// The inquiry's own assistant settings reach the provider.
///
/// Providers read model parameters and the caching policy from the request's
/// stream config, not from the fields `build_inquiry_backend` resolves, so
/// without the config delta every key under
/// `conversation.inquiry.assistant.model.parameters` is addressable,
/// documented, and silently ignored.
#[tokio::test]
async fn llm_backend_applies_its_assistant_config_to_the_request() {
    use jp_config::assistant::request::CachePolicy;

    let inquiry_id = tool_call_inquiry_id("call_params", "confirm", 1);

    let (provider, requests) = structured_provider(json!({ "answer": true })).capturing_requests();

    let mut assistant = PartialAssistantConfig::default();
    assistant.model.parameters.temperature = Some(0.125);
    assistant.request.cache = Some(CachePolicy::Off);

    let config = InquiryConfig {
        assistant,
        ..test_inquiry_config(provider)
    };

    let backend = LlmInquiryBackend::new(config, IndexMap::new(), vec![], vec![]);

    backend
        .inquire(
            test_events(),
            &inquiry_id,
            "test_tool",
            &test_question(),
            CancellationToken::new(),
        )
        .await
        .expect("the inquiry resolves");

    let sent = requests.lock().expect("not poisoned");
    let query = sent.first().expect("one request was sent");
    let resolved = query.thread.events.config().expect("a valid config");

    assert_eq!(
        resolved.assistant.model.parameters.temperature,
        Some(0.125),
        "the inquiry's model parameters reach the provider"
    );
    assert_eq!(
        resolved.assistant.request.cache,
        CachePolicy::Off,
        "and so does its caching policy"
    );
}

#[tokio::test]
async fn llm_backend_returns_error_on_missing_structured_data() {
    let inquiry_id = tool_call_inquiry_id("call_1", "confirm", 1);
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
    let inquiry_id = tool_call_inquiry_id("call_1", "confirm", 1);
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
async fn llm_backend_returns_error_on_null_answer() {
    // A provider returning `{"answer": null}` must fail extraction rather
    // than produce an `Answered { answer: Null }` record.
    let inquiry_id = tool_call_inquiry_id("call_1", "confirm", 1);
    let config = test_inquiry_config(structured_provider(json!({ "answer": null })));
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
    let inquiry_id = tool_call_inquiry_id("call_1", "confirm", 1);

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
    let inquiry_id = tool_call_inquiry_id("call_sel", "choose", 1);
    let question = Question::select("choose", "Pick one")
        .unwrap()
        .with_options(vec!["A".to_string(), "B".to_string()]);
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
    let inquiry_id = tool_call_inquiry_id("call_txt", "reason", 1);
    let question = Question::text("reason", "Why?").unwrap();
    let config = test_inquiry_config(structured_provider(json!({ "answer": "Because reasons" })));
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
    let inquiry_id = tool_call_inquiry_id("call_1", "confirm", 1);
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
    let inquiry_id = tool_call_inquiry_id("call_1", "confirm", 1);
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
    let inquiry_id = tool_call_inquiry_id("call_1", "confirm", 1);
    let default_config = test_inquiry_config(
        // Default provider returns wrong data (would fail extraction).
        structured_provider(json!({ "unrelated": true })),
    );

    let override_config = InquiryConfig {
        assistant: PartialAssistantConfig::default(),
        provider: Arc::new(structured_provider(json!({ "answer": true }))),
        model: test_model(),
        system_prompt: Some("Override prompt.".into()),
        sections: vec![],
        max_response_bytes: Some(1_048_576),
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
/// `ToolCallRequest` and the synthetic `ToolCallResponse`, the old code would
/// place the breakpoint on the non-visible `InquiryRequest`.
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

#[tokio::test]
async fn dedicated_model_backend_returns_answer() {
    let inquiry_id = tool_call_inquiry_id("call_dedicated", "confirm", 1);
    let config = InquiryConfig {
        assistant: PartialAssistantConfig::default(),
        provider: Arc::new(structured_provider(json!({ "answer": true }))),
        model: ModelDetails::empty(ModelIdConfig {
            provider: ProviderId::Test,
            name: "cheap-model".parse().unwrap(),
        }),
        system_prompt: Some("Answer concisely.".to_string()),
        sections: vec![],
        max_response_bytes: Some(1_048_576),
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
