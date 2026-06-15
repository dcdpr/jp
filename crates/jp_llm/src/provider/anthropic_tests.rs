use indexmap::IndexMap;
use jp_config::model::parameters::{
    PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningEffort,
};
use jp_conversation::{event::ChatRequest, thread::Thread};
use jp_test::{Result, function_name};
use serde_json::Map;
use test_log::test;

use super::*;
use crate::test::{TestRequest, run_test};

const MAGIC_STRING: &str = "ANTHROPIC_MAGIC_STRING_TRIGGER_REDACTED_THINKING_46C9A13E193C177646C7398A98432ECCCE4C1253D5E2D82641AC0E52CC2876CB";

#[test(tokio::test)]
async fn test_redacted_thinking() -> Result {
    let requests = vec![
        TestRequest::chat(PROVIDER)
            .enable_reasoning()
            .chat_request(MAGIC_STRING),
        TestRequest::chat(PROVIDER)
            .chat_request("Do you have access to your redacted thinking content?"),
    ];

    run_test(PROVIDER, function_name!(), requests).await
}

#[test(tokio::test)]
async fn test_request_chaining() -> Result {
    let mut request = TestRequest::chat(PROVIDER)
        .reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::Absolute(1024.into())),
                exclude: Some(false),
            },
        )))
        .chat_request("Give me a 2000 word explainer about Kirigami-inspired parachutes");

    if let Some(details) = request.as_model_details_mut() {
        details.max_output_tokens = Some(1152);
    }

    run_test(PROVIDER, function_name!(), Some(request)).await
}

/// Test that Opus 4.6 uses adaptive thinking mode with the effort parameter.
#[test(tokio::test)]
async fn test_opus_4_6_adaptive_thinking() -> Result {
    let mut request = TestRequest::chat(PROVIDER)
        .reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::High),
                exclude: Some(false),
            },
        )))
        .model("anthropic/claude-opus-4-6".parse().unwrap())
        .chat_request("What is 2 + 2?");

    // Configure model to use adaptive thinking (Opus 4.6 feature)
    if let Some(details) = request.as_model_details_mut() {
        details.reasoning = Some(ReasoningDetails::adaptive(false, true));
        details.features = vec!["adaptive-thinking"];
    }

    run_test(PROVIDER, function_name!(), Some(request)).await
}

/// Test Opus 4.6 with `Max` effort level (only supported on Opus 4.6).
#[test(tokio::test)]
async fn test_opus_4_6_max_effort() -> Result {
    let mut request = TestRequest::chat(PROVIDER)
        .reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::Max),
                exclude: Some(false),
            },
        )))
        .model("anthropic/claude-opus-4-6".parse().unwrap())
        .chat_request("What is 2 + 2?");

    // Configure model to use adaptive thinking with max effort support (Opus 4.6 feature)
    if let Some(details) = request.as_model_details_mut() {
        details.reasoning = Some(ReasoningDetails::adaptive(false, true));
        details.features = vec!["adaptive-thinking"];
    }

    run_test(PROVIDER, function_name!(), Some(request)).await
}

/// Unit test: Verify Opus 4.6 generates adaptive thinking request.
#[test]
fn test_opus_4_6_request_uses_adaptive_thinking() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: Some("Claude Opus 4.6".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(false, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["adaptive-thinking"],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, is_structured, _) = create_request(&model, query, true, &beta).unwrap();
    assert!(!is_structured);

    // Verify adaptive thinking is used
    assert_eq!(
        request.thinking,
        Some(types::ExtendedThinking::Adaptive {
            display: Some(types::ThinkingDisplay::Summarized),
        })
    );

    // Verify output_config has effort set (defaults to High)
    assert!(request.output_config.is_some());
    let output_config = request.output_config.unwrap();
    assert_eq!(output_config.effort, Some(Effort::High));
    assert_eq!(output_config.format, None);
}

/// Unit test: Verify `XHigh` effort maps to `Effort::XHigh` for Opus 4.7.
#[test]
fn test_opus_4_7_xhigh_effort_mapping() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-7").try_into().unwrap(),
        display_name: Some("Claude Opus 4.7".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(true, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["adaptive-thinking"],
    };

    let mut events = ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Custom(
        PartialCustomReasoningConfig {
            effort: Some(ReasoningEffort::XHigh),
            exclude: Some(false),
        },
    ));
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    assert_eq!(
        request.thinking,
        Some(types::ExtendedThinking::Adaptive {
            display: Some(types::ThinkingDisplay::Summarized),
        })
    );
    let output_config = request.output_config.unwrap();
    assert_eq!(output_config.effort, Some(Effort::XHigh));
}

/// Unit test: Verify `XHigh` effort falls back to `High` for Opus 4.6 (no xhigh
/// support).
#[test]
fn test_opus_4_6_xhigh_falls_back_to_high() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: Some("Claude Opus 4.6".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(false, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["adaptive-thinking"],
    };

    let mut events = ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Custom(
        PartialCustomReasoningConfig {
            effort: Some(ReasoningEffort::XHigh),
            exclude: Some(false),
        },
    ));
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    let output_config = request.output_config.unwrap();
    assert_eq!(output_config.effort, Some(Effort::High));
}

/// Unit test: Verify Max effort maps to `Effort::Max` for Opus 4.6.
#[test]
fn test_opus_4_6_max_effort_mapping() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: Some("Claude Opus 4.6".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(false, true)), // supports max
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["adaptive-thinking"],
    };

    let mut events = ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Custom(
        PartialCustomReasoningConfig {
            effort: Some(ReasoningEffort::Max),
            exclude: Some(false),
        },
    ));
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    // Verify Max effort is used
    assert!(request.output_config.is_some());
    let output_config = request.output_config.unwrap();
    assert_eq!(output_config.effort, Some(Effort::Max));
}

/// Verify the `map_model` arm for Claude Opus 4.8 produces the expected
/// `ModelDetails`.
/// This is the regression test that catches typos in the declarative model
/// table.
#[test]
fn test_map_model_opus_4_8() {
    let model = types::Model {
        id: "claude-opus-4-8".to_string(),
        display_name: "Claude Opus 4.8".to_string(),
        created_at: String::new(),
        model_type: "model".to_string(),
        max_input_tokens: 0,
        max_tokens: 0,
        capabilities: types::ModelCapabilities::default(),
    };

    let details = map_model(model).unwrap();

    assert_eq!(
        details.id,
        (PROVIDER, "claude-opus-4-8").try_into().unwrap()
    );
    assert_eq!(details.display_name.as_deref(), Some("Claude Opus 4.8"));
    assert_eq!(details.context_window, Some(1_000_000));
    assert_eq!(details.max_output_tokens, Some(128_000));
    assert_eq!(
        details.reasoning,
        Some(ReasoningDetails::adaptive(true, true))
    );
    assert_eq!(details.structured_output, Some(true));
    assert_eq!(details.deprecated, Some(ModelDeprecation::Active));
    assert!(details.features.contains(&"adaptive-thinking"));
    assert!(details.features.contains(&"interleaved-thinking"));
    assert!(details.features.contains(&"context-editing"));
}

/// Verify the `map_model` arm for Claude Fable 5 produces the expected
/// `ModelDetails`, including the `thinking-always-on` capability that stops JP
/// from sending `thinking: disabled` (which Fable rejects).
#[test]
fn test_map_model_fable_5() {
    let model = types::Model {
        id: "claude-fable-5".to_string(),
        display_name: "Claude Fable 5".to_string(),
        created_at: String::new(),
        model_type: "model".to_string(),
        max_input_tokens: 0,
        max_tokens: 0,
        capabilities: types::ModelCapabilities::default(),
    };

    let details = map_model(model).unwrap();

    assert_eq!(details.id, (PROVIDER, "claude-fable-5").try_into().unwrap());
    assert_eq!(details.display_name.as_deref(), Some("Claude Fable 5"));
    assert_eq!(details.context_window, Some(1_000_000));
    assert_eq!(details.max_output_tokens, Some(128_000));
    assert_eq!(
        details.knowledge_cutoff,
        NaiveDate::from_ymd_opt(2026, 1, 1)
    );
    assert_eq!(
        details.reasoning,
        Some(ReasoningDetails::adaptive(true, true))
    );
    assert_eq!(details.structured_output, Some(true));
    assert_eq!(details.deprecated, Some(ModelDeprecation::Active));
    assert!(details.features.contains(&"adaptive-thinking"));
    assert!(details.features.contains(&"interleaved-thinking"));
    assert!(details.features.contains(&"context-editing"));
    assert!(!details.supports_disabling_thinking());
    assert!(!details.supports_prefill());
}

/// Unknown models fall back to the API-reported token limits.
#[test]
fn test_map_model_unknown_uses_api_token_limits() {
    let model = types::Model {
        id: "claude-future-99".to_string(),
        display_name: "Claude Future 99".to_string(),
        created_at: String::new(),
        model_type: "model".to_string(),
        max_input_tokens: 200_000,
        max_tokens: 64_000,
        capabilities: types::ModelCapabilities::default(),
    };

    let details = map_model(model).unwrap();
    assert_eq!(details.max_output_tokens, Some(64_000));
    assert_eq!(details.context_window, Some(200_000));
    // Default capabilities report structured outputs as unsupported.
    assert_eq!(details.structured_output, Some(false));
}

/// A `0` token limit from the API means "unspecified", so it stays unknown and
/// the request later falls back to its default rather than capping at zero.
#[test]
fn test_map_model_unknown_zero_tokens_is_unknown() {
    let model = types::Model {
        id: "claude-future-99".to_string(),
        display_name: "Claude Future 99".to_string(),
        created_at: String::new(),
        model_type: "model".to_string(),
        max_input_tokens: 0,
        max_tokens: 0,
        capabilities: types::ModelCapabilities::default(),
    };

    let details = map_model(model).unwrap();
    assert_eq!(details.max_output_tokens, None);
    assert_eq!(details.context_window, None);
}

/// A `stop_reason: "refusal"` maps to `FinishReason::Refused`, carrying the
/// category and explanation from `stop_details`.
#[test]
fn test_map_message_delta_refusal() {
    let delta: types::MessageDelta = serde_json::from_value(serde_json::json!({
        "stop_reason": "refusal",
        "stop_details": {
            "type": "refusal",
            "category": "cyber",
            "explanation": "This request was declined.",
        },
    }))
    .unwrap();

    assert!(matches!(
        map_message_delta(&delta),
        Some(Event::Finished(FinishReason::Refused { category, explanation }))
            if category.as_deref() == Some("cyber")
                && explanation.as_deref() == Some("This request was declined.")
    ));
}

/// An uncategorized refusal (no `stop_details`) still maps to `Refused`, with
/// both fields `None`.
#[test]
fn test_map_message_delta_refusal_uncategorized() {
    let delta: types::MessageDelta =
        serde_json::from_value(serde_json::json!({ "stop_reason": "refusal" })).unwrap();

    assert!(matches!(
        map_message_delta(&delta),
        Some(Event::Finished(FinishReason::Refused {
            category: None,
            explanation: None,
        }))
    ));
}

/// Fable 5 always runs with adaptive thinking.
/// Even when reasoning is disabled, the request must not send `thinking:
/// disabled`, which the API rejects with a 400 error.
#[test]
fn test_fable_5_reasoning_off_omits_disabled_thinking() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-fable-5").try_into().unwrap(),
        display_name: Some("Claude Fable 5".to_string()),
        context_window: Some(1_000_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(true, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: Some(true),
        features: vec!["adaptive-thinking", "thinking-always-on"],
    };

    let mut events = ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Off);
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    // Thinking-always-on model: the disabled-thinking branch is skipped, so no
    // `thinking` field is sent and the model thinks adaptively.
    assert_eq!(request.thinking, None);
}

/// Unit test: Verify budget-based model (Opus 4.5) still uses Enabled thinking.
#[test]
fn test_opus_4_5_uses_budgetted_thinking() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-5").try_into().unwrap(),
        display_name: Some("Claude Opus 4.5".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["interleaved-thinking"],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    // Verify budget-based thinking is used (not adaptive)
    assert!(matches!(
        request.thinking,
        Some(types::ExtendedThinking::Enabled { .. })
    ));

    // Verify output_config is NOT set for budget-based models
    assert!(request.output_config.is_none());
}

/// Verify structured output sets `output_config.format` when the last event is
/// a `ChatRequest` with a schema.
#[test]
fn test_structured_output_sets_format() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: Some("Claude Sonnet 4.5".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: Some(true),
        features: vec![],
    };

    let schema = Map::from_iter([
        ("type".into(), json!("object")),
        ("properties".into(), json!({"name": {"type": "string"}})),
    ]);

    let events = ConversationStream::new_test().with_turn(ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(schema),
        author: None,
    });

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, is_structured, _) = create_request(&model, query, true, &beta).unwrap();

    assert!(is_structured);
    assert!(request.output_config.is_some());
    let output_config = request.output_config.unwrap();
    // No adaptive thinking, so effort should be None.
    assert_eq!(output_config.effort, None);
    // transform_schema adds additionalProperties: false for objects.
    let expected_schema = Map::from_iter([
        ("type".into(), json!("object")),
        ("properties".into(), json!({"name": {"type": "string"}})),
        ("additionalProperties".into(), json!(false)),
    ]);
    assert_eq!(
        output_config.format,
        Some(JsonOutputFormat::JsonSchema {
            schema: expected_schema
        })
    );
}

/// When the last event is NOT a `ChatRequest` (e.g. a `ChatResponse`), no
/// structured output should be configured even if a prior `ChatRequest` had a
/// schema.
#[test]
fn test_schema_ignored_when_last_event_is_not_chat_request() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec![],
    };

    let mut events = ConversationStream::new_test();

    // First turn: structured request
    events.start_turn(ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(Map::from_iter([("type".into(), json!("object"))])),
        author: None,
    });

    // Then a response (now the last event is not a ChatRequest)
    events
        .current_turn_mut()
        .add_chat_response(ChatResponse::structured(json!({"name": "Alice"})))
        .build()
        .unwrap();

    // Follow-up without schema
    events.start_turn(ChatRequest {
        content: "Explain what you found".into(),
        schema: None,
        author: None,
    });

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (_, is_structured, _) = create_request(&model, query, true, &beta).unwrap();
    assert!(!is_structured);
}

/// Adaptive thinking + structured output should coexist on `OutputConfig`.
#[test]
fn test_adaptive_thinking_with_structured_output() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: Some("Claude Opus 4.6".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(false, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: Some(true),
        features: vec!["adaptive-thinking"],
    };

    let schema = Map::from_iter([("type".into(), json!("object"))]);

    let events = ConversationStream::new_test().with_turn(ChatRequest {
        content: "Extract data".into(),
        schema: Some(schema),
        author: None,
    });

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, is_structured, _) = create_request(&model, query, true, &beta).unwrap();

    assert!(is_structured);
    assert_eq!(
        request.thinking,
        Some(types::ExtendedThinking::Adaptive {
            display: Some(types::ThinkingDisplay::Summarized),
        })
    );

    let output_config = request.output_config.unwrap();
    // Both effort and format should be present.
    assert_eq!(output_config.effort, Some(Effort::High));
    let expected_schema = Map::from_iter([
        ("type".into(), json!("object")),
        ("additionalProperties".into(), json!(false)),
    ]);
    assert_eq!(
        output_config.format,
        Some(JsonOutputFormat::JsonSchema {
            schema: expected_schema
        })
    );
}

/// When reasoning is enabled and `tool_choice` is forced, `create_request`
/// should downgrade to auto + system prompt nudge, and return a
/// `ForcedToolFallback` so `call()` can retry with forced `tool_choice` and
/// thinking disabled.
#[test]
fn test_forced_tool_with_reasoning_returns_fallback() {
    use crate::tool::{ToolDefinition, ToolDocs};

    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: Some("Claude Sonnet 4.5".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec![],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![ToolDefinition {
            name: "my_tool".into(),
            docs: ToolDocs::default(),
            parameters: IndexMap::new(),
        }],
        tool_choice: ToolChoice::Function("my_tool".into()),
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, fallback) = create_request(&model, query, true, &beta).unwrap();

    // tool_choice should have been downgraded to auto.
    assert!(
        matches!(request.tool_choice, Some(types::ToolChoice::Auto { .. })),
        "Expected Auto tool_choice, got {:?}",
        request.tool_choice
    );

    // Single tool + Function gets normalized to Required (→ Any).
    let fallback = fallback.expect("Expected ForcedToolFallback to be Some");
    assert!(
        matches!(fallback.tool_choice, types::ToolChoice::Any { .. }),
        "Expected Any (Required) tool_choice in fallback, got {:?}",
        fallback.tool_choice
    );

    // System prompt should contain the nudge.
    let system = request.system.as_ref().expect("Expected system prompt");
    let system_text = match system {
        types::System::Content(parts) => parts
            .iter()
            .map(|p| match p {
                types::SystemContent::Text(t) => t.text.as_str(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        types::System::String(s) => s.clone(),
    };
    assert!(
        system_text.contains("MUST"),
        "System prompt should contain tool forcing nudge"
    );
}

/// Fable 5 keeps thinking on permanently, so the disable-thinking hard retry
/// isn't available.
/// `create_request` downgrades to a soft-force (auto + system nudge) and sets
/// up an escalating-nudge fallback that keeps thinking on.
#[test]
fn test_forced_tool_thinking_always_on_uses_escalating_nudge() {
    use crate::tool::{ToolDefinition, ToolDocs};

    let model = ModelDetails {
        id: (PROVIDER, "claude-fable-5").try_into().unwrap(),
        display_name: Some("Claude Fable 5".to_string()),
        context_window: Some(1_000_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(true, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: Some(true),
        features: vec!["adaptive-thinking", "thinking-always-on"],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![ToolDefinition {
            name: "my_tool".into(),
            docs: ToolDocs::default(),
            parameters: IndexMap::new(),
        }],
        tool_choice: ToolChoice::Function("my_tool".into()),
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, fallback) = create_request(&model, query, true, &beta).unwrap();

    // Thinking-always-on model uses the escalating-nudge strategy, not the
    // disable-thinking hard retry.
    let fallback = fallback.expect("Expected an escalating-nudge fallback");
    assert!(matches!(
        fallback.strategy,
        ForceStrategy::EscalatingNudge {
            remaining: SOFT_FORCE_MAX_RETRIES
        }
    ));

    // The soft-force downgrade still happens: tool_choice becomes auto and the
    // system prompt carries the nudge.
    assert!(
        matches!(request.tool_choice, Some(types::ToolChoice::Auto { .. })),
        "Expected Auto tool_choice, got {:?}",
        request.tool_choice
    );
    let system = request.system.as_ref().expect("Expected system prompt");
    let system_text = match system {
        types::System::Content(parts) => parts
            .iter()
            .map(|p| match p {
                types::SystemContent::Text(t) => t.text.as_str(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        types::System::String(s) => s.clone(),
    };
    assert!(
        system_text.contains("MUST"),
        "System prompt should contain tool forcing nudge"
    );

    // Thinking stays adaptive, never disabled.
    assert!(matches!(
        request.thinking,
        Some(types::ExtendedThinking::Adaptive { .. })
    ));
}

/// Fable 5 keeps thinking on even when the user sets `reasoning = "off"` (it
/// cannot disable thinking), so a real forced `tool_choice` would still 400.
/// The gate must treat thinking-always-on as active and soft-force regardless
/// of the reasoning config.
#[test]
fn test_forced_tool_thinking_always_on_reasoning_off_still_soft_forces() {
    use crate::tool::{ToolDefinition, ToolDocs};

    let model = ModelDetails {
        id: (PROVIDER, "claude-fable-5").try_into().unwrap(),
        display_name: Some("Claude Fable 5".to_string()),
        context_window: Some(1_000_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(true, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: Some(true),
        features: vec!["adaptive-thinking", "thinking-always-on"],
    };

    let mut events = ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Off);
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![ToolDefinition {
            name: "my_tool".into(),
            docs: ToolDocs::default(),
            parameters: IndexMap::new(),
        }],
        tool_choice: ToolChoice::Function("my_tool".into()),
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, fallback) = create_request(&model, query, true, &beta).unwrap();

    // The forced choice must be downgraded to auto; sending it while thinking is
    // active is the 400 this guards against.
    assert!(
        matches!(request.tool_choice, Some(types::ToolChoice::Auto { .. })),
        "Expected Auto tool_choice, got {:?}",
        request.tool_choice
    );

    // The escalating-nudge fallback is set up (thinking stays on, can't disable).
    let fallback = fallback.expect("Expected an escalating-nudge fallback");
    assert!(matches!(
        fallback.strategy,
        ForceStrategy::EscalatingNudge { .. }
    ));

    // Thinking is never disabled for Fable; with reasoning off the field is
    // omitted and the model thinks adaptively.
    assert_eq!(request.thinking, None);
}

/// With multiple tools, `Function("specific")` is NOT normalized to `Required`
/// and the fallback should carry `Tool { name }` so the retry targets that
/// specific tool.
#[test]
fn test_forced_tool_function_multi_tool_preserves_name() {
    use crate::tool::{ToolDefinition, ToolDocs};

    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: Some("Claude Sonnet 4.5".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec![],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![
            ToolDefinition {
                name: "read_file".into(),
                docs: ToolDocs::default(),
                parameters: IndexMap::new(),
            },
            ToolDefinition {
                name: "commit".into(),
                docs: ToolDocs::default(),
                parameters: IndexMap::new(),
            },
        ],
        tool_choice: ToolChoice::Function("commit".into()),
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, fallback) = create_request(&model, query, true, &beta).unwrap();

    assert!(
        matches!(request.tool_choice, Some(types::ToolChoice::Auto { .. })),
        "Expected Auto tool_choice, got {:?}",
        request.tool_choice
    );

    let fallback = fallback.expect("Expected ForcedToolFallback to be Some");
    assert!(
        matches!(fallback.tool_choice, types::ToolChoice::Tool { ref name, .. } if name == "commit"),
        "Expected Tool {{ name: \"commit\" }} in fallback, got {:?}",
        fallback.tool_choice
    );

    // is_satisfied_by should only accept the specific tool.
    assert!(!fallback.is_satisfied_by(&[]));
    assert!(!fallback.is_satisfied_by(&["read_file".into()]));
    assert!(fallback.is_satisfied_by(&["commit".into()]));
    assert!(fallback.is_satisfied_by(&["read_file".into(), "commit".into()]));
}

/// `is_satisfied_by` for `Any` (Required) accepts any tool call.
#[test]
fn test_fallback_any_satisfied_by_any_tool() {
    let fb = ForcedToolFallback {
        tool_choice: types::ToolChoice::any(),
        strategy: ForceStrategy::DisableThinking,
    };
    assert!(!fb.is_satisfied_by(&[]));
    assert!(fb.is_satisfied_by(&["whatever".into()]));
}

/// Without reasoning, forced `tool_choice` should NOT produce a fallback.
#[test]
fn test_forced_tool_without_reasoning_no_fallback() {
    use crate::tool::{ToolDefinition, ToolDocs};

    let model = ModelDetails {
        id: (PROVIDER, "claude-3-haiku-20240307").try_into().unwrap(),
        display_name: Some("Claude 3 Haiku".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(4_096),
        reasoning: Some(ReasoningDetails::unsupported()),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec![],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![ToolDefinition {
            name: "my_tool".into(),
            docs: ToolDocs::default(),
            parameters: IndexMap::new(),
        }],
        tool_choice: ToolChoice::Required,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, fallback) = create_request(&model, query, true, &beta).unwrap();

    // No fallback needed - tool_choice should stay as forced (any).
    assert!(fallback.is_none(), "Expected no fallback without reasoning");
    assert!(
        matches!(request.tool_choice, Some(types::ToolChoice::Any { .. })),
        "Expected Any tool_choice, got {:?}",
        request.tool_choice
    );
}

/// With reasoning + auto `tool_choice`, no fallback should be produced.
#[test]
fn test_auto_tool_choice_with_reasoning_no_fallback() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: Some("Claude Sonnet 4.5".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec![],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (_, _, fallback) = create_request(&model, query, true, &beta).unwrap();

    assert!(
        fallback.is_none(),
        "Expected no fallback with auto tool_choice"
    );
}

#[test]
fn test_find_merge_point_edge_cases() {
    struct TestCase {
        left: &'static str,
        right: &'static str,
        expected: &'static str,
        max_search: usize,
    }

    let cases = IndexMap::from([
        ("no overlap", TestCase {
            left: "Hello",
            right: " world",
            expected: "Hello world",
            max_search: 500,
        }),
        ("single word overlap", TestCase {
            left: "The quick brown",
            right: "brown fox",
            expected: "The quick brown fox",
            max_search: 500,
        }),
        ("minimal overlap (5 chars)", TestCase {
            expected: "abcdefghij",
            left: "abcdefgh",
            right: "defghij",
            max_search: 500,
        }),
        (
            "below minimum overlap (4 chars) - should not merge",
            TestCase {
                left: "abcd",
                right: "abcd",
                expected: "abcdabcd",
                max_search: 500,
            },
        ),
        ("complete overlap", TestCase {
            left: "Hello world",
            right: "world",
            expected: "Hello world",
            max_search: 500,
        }),
        ("overlap with punctuation", TestCase {
            left: "Hello, how are",
            right: "how are you?",
            expected: "Hello, how are you?",
            max_search: 500,
        }),
        ("overlap with whitespace", TestCase {
            left: "Hello     ",
            right: "     world",
            expected: "Hello     world",
            max_search: 500,
        }),
        ("unicode overlap", TestCase {
            left: "Hello 世界",
            right: "世界 friend",
            expected: "Hello 世界 friend",
            max_search: 500,
        }),
        ("long overlap", TestCase {
            left: "The quick brown fox jumps",
            right: "fox jumps over the lazy dog",
            expected: "The quick brown fox jumpsfox jumps over the lazy dog",
            max_search: 8,
        }),
        ("empty right", TestCase {
            left: "Hello",
            right: "",
            expected: "Hello",
            max_search: 500,
        }),
    ]);

    let min_overlap = 5;
    for (
        name,
        TestCase {
            left,
            right,
            expected,
            max_search,
        },
    ) in cases
    {
        let pos = find_merge_point(left, right, max_search, min_overlap);
        let result = format!("{left}{}", &right[pos..]);
        assert_eq!(result, expected, "Failed test case: {name}");
    }
}

/// When the last event is an assistant message and the model does NOT have the
/// "prefill" feature, a synthetic user "continue" message is appended.
#[test]
fn test_continue_injected_when_prefill_unsupported() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(false, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        // No "prefill" feature.
        features: vec!["adaptive-thinking"],
    };

    let mut events = ConversationStream::new_test();
    events.start_turn(ChatRequest {
        content: "Tell me about X".into(),
        schema: None,
        author: None,
    });
    events
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("X is a topic that was first"))
        .build()
        .unwrap();

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    // Last message should be the synthetic continue message.
    let last = request.messages.last().unwrap();
    assert_eq!(last.role, types::MessageRole::User);
    assert_eq!(request.messages.len(), 3); // user, assistant, synthetic user
}

/// When the model HAS the "prefill" feature, no synthetic message is injected
/// even if the last event is an assistant message.
#[test]
fn test_prefill_preserved_for_supported_models() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["interleaved-thinking", "prefill"],
    };

    let mut events = ConversationStream::new_test();
    events.start_turn(ChatRequest {
        content: "Tell me about X".into(),
        schema: None,
        author: None,
    });
    events
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("X is a topic that was first"))
        .build()
        .unwrap();

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    // Last message should be the assistant message (prefill), not a synthetic user message.
    let last = request.messages.last().unwrap();
    assert_eq!(last.role, types::MessageRole::Assistant);
    assert_eq!(request.messages.len(), 2); // user, assistant
}

/// Normal flow: last event is a user message.
/// No injection needed regardless of prefill support.
#[test]
fn test_no_injection_when_last_message_is_user() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(false, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["adaptive-thinking"],
    };

    let events = ConversationStream::new_test().with_turn("What is 2+2?");

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    let last = request.messages.last().unwrap();
    assert_eq!(last.role, types::MessageRole::User);
    assert_eq!(request.messages.len(), 1); // just the user message
}

#[test]
fn test_create_request_resends_signed_thinking_as_native_block() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["prefill"],
    };

    let mut events = ConversationStream::new_test();
    events.start_turn("First question");
    events.extend([
        ConversationEvent::now(ChatResponse::reasoning("internal reasoning"))
            .with_metadata_field(THINKING_SIGNATURE_KEY, "sig_123"),
        ConversationEvent::now(ChatResponse::message("Visible answer")),
    ]);
    events.start_turn("Follow-up question");

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    assert_eq!(request.messages.len(), 3);
    let assistant = &request.messages[1];
    assert_eq!(assistant.role, types::MessageRole::Assistant);
    assert_eq!(assistant.content.0.len(), 2);

    assert!(matches!(
        &assistant.content.0[0],
        types::MessageContent::Thinking(types::Thinking {
            thinking,
            signature,
        }) if thinking == "internal reasoning" && signature.as_deref() == Some("sig_123")
    ));
    assert!(matches!(
        &assistant.content.0[1],
        types::MessageContent::Text(text) if text.text == "Visible answer"
    ));
}

#[test]
fn test_create_request_resends_redacted_thinking_as_native_block() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["prefill"],
    };

    let mut events = ConversationStream::new_test();
    events.start_turn("First question");
    events.extend([
        ConversationEvent::now(ChatResponse::reasoning(""))
            .with_metadata_field(REDACTED_THINKING_KEY, "encrypted_payload"),
        ConversationEvent::now(ChatResponse::message("Visible answer")),
    ]);
    events.start_turn("Follow-up question");

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    assert_eq!(request.messages.len(), 3);
    let assistant = &request.messages[1];
    assert_eq!(assistant.role, types::MessageRole::Assistant);
    assert_eq!(assistant.content.0.len(), 2);

    assert!(matches!(
        &assistant.content.0[0],
        types::MessageContent::RedactedThinking { data } if data == "encrypted_payload"
    ));
    assert!(matches!(
        &assistant.content.0[1],
        types::MessageContent::Text(text) if text.text == "Visible answer"
    ));
}

#[test]
fn test_create_request_falls_back_to_think_tags_without_signature() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        features: vec!["prefill"],
    };

    let mut events = ConversationStream::new_test();
    events.start_turn("First question");
    events.extend([
        ConversationEvent::now(ChatResponse::reasoning("internal reasoning")),
        ConversationEvent::now(ChatResponse::message("Visible answer")),
    ]);
    events.start_turn("Follow-up question");

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    assert_eq!(request.messages.len(), 3);
    let assistant = &request.messages[1];
    assert_eq!(assistant.role, types::MessageRole::Assistant);
    assert_eq!(assistant.content.0.len(), 2);

    assert!(matches!(
        &assistant.content.0[0],
        types::MessageContent::Text(text)
            if text.text == "<think>\ninternal reasoning\n</think>\n\n"
    ));
    assert!(matches!(
        &assistant.content.0[1],
        types::MessageContent::Text(text) if text.text == "Visible answer"
    ));
}

/// When the conversation ends with an assistant turn carrying signed thinking
/// (a resumed/continued turn), that thinking must be rewritten as `<think>`
/// text rather than re-sent as a native block, which Anthropic rejects in the
/// continuation target.
#[test]
fn test_create_request_downgrades_trailing_assistant_thinking() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(false, true)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        // No "prefill" feature, so a synthetic continue is appended after the
        // (downgraded) assistant turn.
        features: vec!["adaptive-thinking"],
    };

    let mut events = ConversationStream::new_test();
    events.start_turn("Question");
    events.extend([
        ConversationEvent::now(ChatResponse::reasoning("internal reasoning"))
            .with_metadata_field(THINKING_SIGNATURE_KEY, "sig_123"),
        ConversationEvent::now(ChatResponse::message("partial answer")),
    ]);

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    // user, assistant (downgraded), synthetic continue user.
    assert_eq!(request.messages.len(), 3);
    let assistant = &request.messages[1];
    assert_eq!(assistant.role, types::MessageRole::Assistant);

    assert!(
        !assistant
            .content
            .0
            .iter()
            .any(|c| matches!(c, types::MessageContent::Thinking(_))),
        "trailing assistant thinking must be downgraded, not sent natively"
    );
    assert!(matches!(
        &assistant.content.0[0],
        types::MessageContent::Text(text)
            if text.text == "<think>\ninternal reasoning\n</think>\n\n"
    ));
    assert!(matches!(
        &assistant.content.0[1],
        types::MessageContent::Text(text) if text.text == "partial answer"
    ));
}

/// Redacted (encrypted) thinking in the continuation target has no readable
/// form, so it is dropped rather than rewritten.
#[test]
fn test_create_request_drops_trailing_redacted_thinking() {
    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        // "prefill" keeps the assistant message as the trailing continuation
        // target (no synthetic continue).
        features: vec!["prefill"],
    };

    let mut events = ConversationStream::new_test();
    events.start_turn("Question");
    events.extend([
        ConversationEvent::now(ChatResponse::reasoning(""))
            .with_metadata_field(REDACTED_THINKING_KEY, "encrypted_payload"),
        ConversationEvent::now(ChatResponse::message("partial answer")),
    ]);

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _, _) = create_request(&model, query, true, &beta).unwrap();

    let assistant = request.messages.last().unwrap();
    assert_eq!(assistant.role, types::MessageRole::Assistant);
    assert_eq!(assistant.content.0.len(), 1, "redacted thinking is dropped");
    assert!(matches!(
        &assistant.content.0[0],
        types::MessageContent::Text(text) if text.text == "partial answer"
    ));
}

mod transform_schema {
    use serde_json::{Map, Value, json};

    use super::transform_schema;

    #[expect(clippy::needless_pass_by_value)]
    fn schema(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn object_forces_additional_properties_false() {
        let input = schema(json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }));

        let out = transform_schema(input);

        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(out["required"], json!(["name"]));
        assert_eq!(out["properties"]["name"]["type"], "string");
    }

    #[test]
    fn object_drops_existing_additional_properties() {
        let input = schema(json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        }));

        let out = transform_schema(input);
        assert_eq!(out["additionalProperties"], json!(false));
    }

    #[test]
    fn array_keeps_min_items_0_and_1() {
        for n in [0, 1] {
            let input = schema(json!({
                "type": "array",
                "items": { "type": "string" },
                "minItems": n
            }));

            let out = transform_schema(input);
            assert_eq!(out["minItems"], json!(n), "minItems {n} should be kept");
        }
    }

    #[test]
    fn array_moves_large_min_items_to_description() {
        let input = schema(json!({
            "type": "array",
            "items": { "type": "string" },
            "minItems": 3
        }));

        let out = transform_schema(input);
        assert!(out.get("minItems").is_none());
        let desc = out["description"].as_str().unwrap();
        assert!(
            desc.contains("minItems"),
            "description should mention minItems: {desc}"
        );
    }

    #[test]
    fn array_moves_max_items_to_description() {
        let input = schema(json!({
            "type": "array",
            "items": { "type": "string" },
            "maxItems": 5
        }));

        let out = transform_schema(input);
        assert!(out.get("maxItems").is_none());
        let desc = out["description"].as_str().unwrap();
        assert!(
            desc.contains("maxItems"),
            "description should mention maxItems: {desc}"
        );
    }

    #[test]
    fn string_keeps_supported_format() {
        let input = schema(json!({
            "type": "string",
            "format": "date-time"
        }));

        let out = transform_schema(input);
        assert_eq!(out["format"], "date-time");
        assert!(out.get("description").is_none());
    }

    #[test]
    fn string_moves_unsupported_format_to_description() {
        let input = schema(json!({
            "type": "string",
            "format": "phone-number"
        }));

        let out = transform_schema(input);
        assert!(out.get("format").is_none());
        let desc = out["description"].as_str().unwrap();
        assert!(
            desc.contains("phone-number"),
            "description should contain the format: {desc}"
        );
    }

    #[test]
    fn numeric_constraints_moved_to_description() {
        let input = schema(json!({
            "type": "integer",
            "minimum": 1,
            "maximum": 10,
            "description": "A number"
        }));

        let out = transform_schema(input);
        assert!(out.get("minimum").is_none());
        assert!(out.get("maximum").is_none());
        let desc = out["description"].as_str().unwrap();
        assert!(
            desc.starts_with("A number"),
            "should preserve original description"
        );
        assert!(desc.contains("minimum"), "should contain minimum: {desc}");
        assert!(desc.contains("maximum"), "should contain maximum: {desc}");
    }

    #[test]
    fn ref_passes_through() {
        let input = schema(json!({
            "$ref": "#/$defs/Address"
        }));

        let out = transform_schema(input);
        assert_eq!(out["$ref"], "#/$defs/Address");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn defs_recursively_transformed() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "addr": { "$ref": "#/$defs/Address" }
            },
            "$defs": {
                "Address": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "additionalProperties": true
                }
            }
        }));

        let out = transform_schema(input);
        let addr_def = out["$defs"]["Address"].as_object().unwrap();
        assert_eq!(addr_def["additionalProperties"], json!(false));
    }

    #[test]
    fn one_of_converted_to_any_of() {
        let input = schema(json!({
            "oneOf": [
                { "type": "string" },
                { "type": "integer" }
            ]
        }));

        let out = transform_schema(input);
        assert!(out.get("oneOf").is_none());
        let any_of = out["anyOf"].as_array().unwrap();
        assert_eq!(any_of.len(), 2);
        assert_eq!(any_of[0]["type"], "string");
        assert_eq!(any_of[1]["type"], "integer");
    }

    #[test]
    fn nested_properties_recursively_transformed() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "id": { "type": "integer", "minimum": 0 } },
                        "additionalProperties": true
                    },
                    "maxItems": 10
                }
            }
        }));

        let out = transform_schema(input);

        // Top-level object
        assert_eq!(out["additionalProperties"], json!(false));

        // The array property
        let items_prop = out["properties"]["items"].as_object().unwrap();
        assert!(items_prop.get("maxItems").is_none());

        // The nested object inside the array
        let nested = items_prop["items"].as_object().unwrap();
        assert_eq!(nested["additionalProperties"], json!(false));

        // The integer property's minimum should be in description
        let id_prop = nested["properties"]["id"].as_object().unwrap();
        assert!(id_prop.get("minimum").is_none());
        let desc = id_prop["description"].as_str().unwrap();
        assert!(
            desc.contains("minimum"),
            "nested constraint in description: {desc}"
        );
    }

    /// Mirrors the example from Anthropic's Python SDK docstring.
    #[test]
    fn sdk_docstring_example() {
        let input = schema(json!({
            "type": "integer",
            "minimum": 1,
            "maximum": 10,
            "description": "A number"
        }));

        let out = transform_schema(input);
        assert_eq!(out["type"], "integer");
        let desc = out["description"].as_str().unwrap();
        assert!(desc.starts_with("A number"));
        assert!(desc.contains("minimum: 1"));
        assert!(desc.contains("maximum: 10"));
    }

    /// The `title_schema` used by the title generator should survive
    /// transformation.
    #[test]
    fn title_schema_transforms_cleanly() {
        let input = crate::title::title_schema(3);
        let out = transform_schema(input);

        assert_eq!(out["type"], "object");
        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(out["required"], json!(["titles"]));

        let titles = out["properties"]["titles"].as_object().unwrap();
        assert_eq!(titles["type"], "array");
        // minItems 1 is kept but > 1 is moved to description
        assert!(titles.get("minItems").is_none());
        assert!(titles.get("maxItems").is_none());
        let desc = titles["description"].as_str().unwrap();
        assert!(
            desc.contains("minItems"),
            "should contain minItems hint: {desc}"
        );
        assert!(
            desc.contains("maxItems"),
            "should contain maxItems hint: {desc}"
        );
    }
}

mod thinking_signature_recovery {
    use async_anthropic::types::{self, MessageRole};

    use crate::{
        error::StreamError,
        event::{EventMatcher, PatchAction},
        provider::anthropic::{
            build_thinking_patches, find_oldest_thinking_block, identify_thinking_block,
            is_invalid_thinking_signature, parse_signature_error_position, resolve_turn_position,
        },
    };

    fn make_thinking(text: &str, sig: &str) -> types::MessageContent {
        types::MessageContent::Thinking(types::Thinking {
            thinking: text.to_owned(),
            signature: Some(sig.to_owned()),
        })
    }

    fn make_redacted(data: &str) -> types::MessageContent {
        types::MessageContent::RedactedThinking {
            data: data.to_owned(),
        }
    }

    fn make_text(text: &str) -> types::MessageContent {
        types::MessageContent::Text(text.into())
    }

    fn msg(role: MessageRole, content: Vec<types::MessageContent>) -> types::Message {
        types::Message {
            role,
            content: types::MessageContentList(content),
        }
    }

    fn request(messages: Vec<types::Message>) -> types::CreateMessagesRequest {
        types::CreateMessagesRequestBuilder::default()
            .model("claude-test")
            .messages(messages)
            .build()
            .unwrap()
    }

    fn make_tool_use(id: &str) -> types::MessageContent {
        types::MessageContent::ToolUse(types::ToolUse {
            id: id.to_owned(),
            name: "tool".to_owned(),
            input: serde_json::Value::Null,
            cache_control: None,
        })
    }

    fn make_tool_result(id: &str) -> types::MessageContent {
        types::MessageContent::ToolResult(types::ToolResult {
            tool_use_id: id.to_owned(),
            content: Some("ok".to_owned()),
            is_error: false,
            cache_control: None,
        })
    }

    #[test]
    fn parse_exact_position() {
        let msg = "api error: invalid_request_error: messages.1.content.0: Invalid `signature` in \
                   `thinking` block";
        assert_eq!(parse_signature_error_position(msg), Some((1, 0)));
    }

    #[test]
    fn parse_larger_indices() {
        let msg = "api error: invalid_request_error: messages.12.content.3: Invalid `signature` \
                   in `thinking` block";
        assert_eq!(parse_signature_error_position(msg), Some((12, 3)));
    }

    #[test]
    fn parse_returns_none_for_unrelated_error() {
        assert_eq!(
            parse_signature_error_position("api error: rate_limit_error: too many requests"),
            None
        );
    }

    #[test]
    fn parse_returns_none_for_missing_content() {
        assert_eq!(
            parse_signature_error_position("messages.1: something else"),
            None
        );
    }

    #[test]
    fn detects_signature_error() {
        let error = StreamError::other(
            "api error: invalid_request_error: messages.1.content.0: Invalid `signature` in \
             `thinking` block",
        );
        assert!(is_invalid_thinking_signature(&error));
    }

    #[test]
    fn ignores_unrelated_errors() {
        let error = StreamError::other("api error: rate_limit_error: too many requests");
        assert!(!is_invalid_thinking_signature(&error));
    }

    #[test]
    fn ignores_retryable_errors() {
        let error = StreamError::transient("server error with signature and thinking");
        assert!(!is_invalid_thinking_signature(&error));
    }

    #[test]
    fn finds_first_thinking_block() {
        let request = request(vec![
            msg(MessageRole::User, vec![make_text("hello")]),
            msg(MessageRole::Assistant, vec![
                make_text("preamble"),
                make_thinking("deep thought", "sig_1"),
            ]),
            msg(MessageRole::Assistant, vec![make_thinking(
                "later thought",
                "sig_2",
            )]),
        ]);

        assert_eq!(find_oldest_thinking_block(&request), Some((1, 1)));
    }

    #[test]
    fn finds_redacted_before_thinking_if_older() {
        let request = request(vec![
            msg(MessageRole::User, vec![make_text("hello")]),
            msg(MessageRole::Assistant, vec![
                make_redacted("secret"),
                make_thinking("visible", "sig"),
            ]),
        ]);

        // RedactedThinking at (1, 0) comes before Thinking at (1, 1)
        assert_eq!(find_oldest_thinking_block(&request), Some((1, 0)));
    }

    #[test]
    fn returns_none_without_thinking() {
        let request = request(vec![
            msg(MessageRole::User, vec![make_text("hello")]),
            msg(MessageRole::Assistant, vec![make_text("world")]),
        ]);

        assert_eq!(find_oldest_thinking_block(&request), None);
    }

    #[test]
    fn identifies_thinking_block() {
        let request = request(vec![msg(MessageRole::Assistant, vec![
            make_thinking("my reasoning", "sig_old"),
            make_text("my answer"),
        ])]);

        let result = identify_thinking_block(&request, 0, 0);
        assert_eq!(
            result.as_ref().map(|(k, v)| (*k, v.as_str())),
            Some(("anthropic_thinking_signature", "sig_old"))
        );
    }

    #[test]
    fn identifies_redacted_block() {
        let request = request(vec![msg(MessageRole::Assistant, vec![
            make_redacted("encrypted"),
            make_text("visible"),
        ])]);

        let result = identify_thinking_block(&request, 0, 0);
        assert_eq!(
            result.as_ref().map(|(k, v)| (*k, v.as_str())),
            Some(("anthropic_redacted_thinking", "encrypted"))
        );
    }

    #[test]
    fn identify_out_of_bounds_is_none() {
        let request = request(vec![msg(MessageRole::Assistant, vec![make_thinking(
            "thought", "sig",
        )])]);

        assert!(identify_thinking_block(&request, 99, 0).is_none());
        assert!(identify_thinking_block(&request, 0, 99).is_none());
    }

    #[test]
    fn identify_non_thinking_is_none() {
        let request = request(vec![msg(MessageRole::Assistant, vec![make_text(
            "just text",
        )])]);

        assert!(identify_thinking_block(&request, 0, 0).is_none());
    }

    #[test]
    fn build_patches_from_position_in_error() {
        let request = request(vec![
            msg(MessageRole::User, vec![make_text("hello")]),
            msg(MessageRole::Assistant, vec![
                make_thinking("thought", "sig_bad"),
                make_text("response"),
            ]),
        ]);

        let error = StreamError::other(
            "api error: invalid_request_error: messages.1.content.0: Invalid `signature` in \
             `thinking` block",
        );

        let patches = build_thinking_patches(&request, &error).unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].matcher, EventMatcher::MetadataValue {
            key: "anthropic_thinking_signature".to_owned(),
            value: "sig_bad".to_owned(),
        });
        assert_eq!(
            patches[0].action,
            PatchAction::RemoveMetadata("anthropic_thinking_signature".to_owned())
        );
    }

    #[test]
    fn build_patches_falls_back_to_oldest() {
        let request = request(vec![
            msg(MessageRole::User, vec![make_text("hello")]),
            msg(MessageRole::Assistant, vec![
                make_thinking("thought", "sig_oldest"),
                make_text("response"),
            ]),
        ]);

        // Unparsable position in error, falls back to oldest thinking block.
        let error = StreamError::other(
            "api error: invalid_request_error: Invalid `signature` in `thinking` block",
        );

        let patches = build_thinking_patches(&request, &error).unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].matcher, EventMatcher::MetadataValue {
            key: "anthropic_thinking_signature".to_owned(),
            value: "sig_oldest".to_owned(),
        });
    }

    #[test]
    fn build_patches_none_without_thinking() {
        let request = request(vec![
            msg(MessageRole::User, vec![make_text("hello")]),
            msg(MessageRole::Assistant, vec![make_text("world")]),
        ]);

        let error = StreamError::other(
            "api error: invalid_request_error: messages.1.content.0: Invalid `signature` in \
             `thinking` block",
        );

        assert!(build_thinking_patches(&request, &error).is_none());
    }

    /// Reproduce the exact message structure from the user's failing request:
    /// two assistant turns with tool-use loops, separated by a user message.
    fn tool_use_conversation() -> Vec<types::Message> {
        vec![
            // Turn 0: user
            msg(MessageRole::User, vec![make_text("hello")]),
            // Turn 1: assistant (tool-use loop spanning messages 1-7)
            msg(MessageRole::Assistant, vec![
                make_thinking("t1", "s1"),
                make_text("resp1"),
                make_tool_use("tu1"),
            ]),
            msg(MessageRole::User, vec![make_tool_result("tu1")]),
            msg(MessageRole::Assistant, vec![
                make_thinking("t2", "s2"),
                make_tool_use("tu2"),
            ]),
            msg(MessageRole::User, vec![make_tool_result("tu2")]),
            msg(MessageRole::Assistant, vec![
                make_thinking("t3", "s3"),
                make_tool_use("tu3"),
            ]),
            msg(MessageRole::User, vec![make_tool_result("tu3")]),
            msg(MessageRole::Assistant, vec![
                make_thinking("t4", "s4"),
                make_text("final1"),
            ]),
            // Turn 2: user
            msg(MessageRole::User, vec![make_text("follow up")]),
            // Turn 3: assistant (tool-use loop spanning messages 9-13)
            msg(MessageRole::Assistant, vec![
                make_thinking("t5", "s5"),
                make_text("resp2"),
                make_tool_use("tu4"),
            ]),
            msg(MessageRole::User, vec![make_tool_result("tu4")]),
            msg(MessageRole::Assistant, vec![
                make_thinking("t6", "s6"),
                make_text("resp3"),
                make_tool_use("tu5"),
            ]),
            msg(MessageRole::User, vec![make_tool_result("tu5")]),
            msg(MessageRole::Assistant, vec![
                make_thinking("t7", "s7"),
                make_text("final2"),
            ]),
            // Turn 4: user
            msg(MessageRole::User, vec![make_text("last question")]),
        ]
    }

    #[test]
    fn resolve_turn1_thinking_blocks() {
        let msgs = tool_use_conversation();

        // Turn 1 flattened: [Thinking(0), Text(1), ToolUse(2), ToolResult(3),
        //   Thinking(4), ToolUse(5), ToolResult(6), Thinking(7), ToolUse(8),
        //   ToolResult(9), Thinking(10), Text(11)]
        assert_eq!(resolve_turn_position(&msgs, 1, 0), Some((1, 0))); // t1
        assert_eq!(resolve_turn_position(&msgs, 1, 4), Some((3, 0))); // t2
        assert_eq!(resolve_turn_position(&msgs, 1, 7), Some((5, 0))); // t3
        assert_eq!(resolve_turn_position(&msgs, 1, 10), Some((7, 0))); // t4
    }

    #[test]
    fn resolve_turn3_thinking_blocks() {
        let msgs = tool_use_conversation();

        // Turn 3 flattened: [Thinking(0), Text(1), ToolUse(2), ToolResult(3),
        //   Thinking(4), Text(5), ToolUse(6), ToolResult(7), Thinking(8),
        //   Text(9)]
        assert_eq!(resolve_turn_position(&msgs, 3, 0), Some((9, 0))); // t5
        assert_eq!(resolve_turn_position(&msgs, 3, 4), Some((11, 0))); // t6
        assert_eq!(resolve_turn_position(&msgs, 3, 8), Some((13, 0))); // t7
    }

    #[test]
    fn resolve_user_turn() {
        let msgs = tool_use_conversation();

        // Turn 0 is a single user message.
        assert_eq!(resolve_turn_position(&msgs, 0, 0), Some((0, 0)));
        assert_eq!(resolve_turn_position(&msgs, 0, 1), None);
    }

    #[test]
    fn resolve_out_of_bounds() {
        let msgs = tool_use_conversation();

        assert_eq!(resolve_turn_position(&msgs, 99, 0), None);
        assert_eq!(resolve_turn_position(&msgs, 1, 999), None);
    }

    #[test]
    fn resolve_non_thinking_blocks() {
        let msgs = tool_use_conversation();

        // Turn 1, flat index 1 = Text block in messages[1]
        assert_eq!(resolve_turn_position(&msgs, 1, 1), Some((1, 1)));
        // Turn 1, flat index 2 = ToolUse in messages[1]
        assert_eq!(resolve_turn_position(&msgs, 1, 2), Some((1, 2)));
        // Turn 1, flat index 3 = ToolResult in messages[2]
        assert_eq!(resolve_turn_position(&msgs, 1, 3), Some((2, 0)));
    }
}
