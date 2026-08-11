use indexmap::IndexMap;
use jp_config::{conversation::tool::ToolParameterConfig, providers::llm::LlmProviderConfig};
use jp_test::{Result, function_name};

use super::*;
use crate::test::TestRequest;

macro_rules! test_all_models {
        ($($fn:ident),* $(,)?) => {
            mod anthropic { use super::*; $(test_all_models!(func; $fn, "openrouter/anthropic/claude-haiku-4.5");)* }
            mod google    { use super::*; $(test_all_models!(func; $fn, "openrouter/google/gemini-2.5-flash");)* }
            mod xai       { use super::*; $(test_all_models!(func; $fn, "openrouter/x-ai/grok-code-fast-1");)* }
            mod minimax   { use super::*; $(test_all_models!(func; $fn, "openrouter/minimax/minimax-m2");)* }
        };
        (func; $fn:ident, $model:literal) => {
            paste::paste! {
                #[test_log::test(tokio::test)]
                async fn [< test_ $fn >]() -> Result {
                    $fn($model, &format!("{}_{}", $model.split('/').nth(1).unwrap(), function_name!())).await
                }
            }
        };
    }

test_all_models![sub_provider_event_metadata];

async fn sub_provider_event_metadata(model: &str, test_name: &str) -> Result {
    let requests = vec![
        TestRequest::chat(ProviderId::Openrouter)
            .model(model.parse().unwrap())
            .enable_reasoning()
            .chat_request("Test message"),
    ];

    run_test(test_name, requests).await?;

    Ok(())
}

#[test]
fn request_preserves_integer_tool_parameter_type() -> Result {
    let request = TestRequest::chat(ProviderId::Openrouter)
        .tool("fs_read_file", [("start_line", ToolParameterConfig {
            kind: "integer".to_owned().into(),
            required: false,
            default: None,
            summary: None,
            description: None,
            examples: None,
            enumeration: vec![],
            items: None,
            properties: IndexMap::new(),
        })])
        .chat_request("Read README.md");
    let TestRequest::Chat { model, query, .. } = request else {
        unreachable!();
    };

    let (request, _) = create_request(&model, query)?;
    let request = serde_json::to_value(request)?;

    assert_eq!(
        request["tools"][0]["function"],
        serde_json::json!({
            "name": "fs_read_file",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "start_line": {"type": ["integer", "null"]}
                },
                "additionalProperties": false,
                "required": ["start_line"]
            }
        })
    );
    Ok(())
}

/// A tool-call finish closes the provider stream without ending the Turn.
#[test]
fn tool_call_finish_is_a_clean_completion() -> Result {
    let tool_call: response::Choice = serde_json::from_value(serde_json::json!({
        "finish_reason": null,
        "native_finish_reason": null,
        "delta": {
            "role": "assistant",
            "content": null,
            "reasoning": null,
            "tool_calls": [{
                "index": 0,
                "id": "call_1",
                "function": {
                    "name": "fs_read_file",
                    "arguments": "{\"path\":\"README.md\",\"start_line\":\"1\"}"
                }
            }]
        },
        "error": null
    }))?;
    let finish: response::Choice = serde_json::from_value(serde_json::json!({
        "finish_reason": "tool_calls",
        "native_finish_reason": "tool_calls",
        "delta": {
            "role": null,
            "content": null,
            "reasoning": null,
            "tool_calls": []
        },
        "error": null
    }))?;
    let mut state = AggregationState {
        tool_call_indices: vec![],
        aggregating_reasoning: false,
        aggregating_message: false,
        is_structured: false,
    };

    let events = map_event(tool_call, &mut state)
        .into_iter()
        .chain(map_event(finish, &mut state))
        .collect::<std::result::Result<Vec<_>, _>>()?;

    assert_eq!(events, vec![
        Event::tool_call_start(2, "call_1", "fs_read_file"),
        Event::tool_call_args(2, r#"{"path":"README.md","start_line":"1"}"#),
        Event::flush(2),
        Event::Finished(event::FinishReason::Completed),
    ]);
    Ok(())
}

/// Capabilities come from the catalog rather than being left unknown.
/// Leaving them unknown is what made unregistered models silently fall back to
/// provider defaults.
#[test]
fn test_map_model_derives_capabilities() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "anthropic/claude-opus-5-fast",
        "name": "Claude Opus 5 (Fast)",
        "created": 1_784_912_546,
        "context_length": 1_000_000,
        "top_provider": {
            "context_length": 1_000_000,
            "max_completion_tokens": 128_000,
            "is_moderated": true,
        },
        "supported_parameters": [
            "include_reasoning", "max_tokens", "reasoning", "reasoning_effort",
            "response_format", "stop", "structured_outputs", "tool_choice",
            "tools", "verbosity",
        ],
        "reasoning": {
            "mandatory": false,
            "default_enabled": true,
            "supported_efforts": ["max", "xhigh", "high", "medium", "low"],
            "default_effort": "high",
        },
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.context_window, Some(1_000_000));
    assert_eq!(details.max_output_tokens, Some(128_000));
    assert_eq!(details.structured_output, Some(true));
    assert_eq!(
        details.reasoning,
        Some(ModelReasoningDetails::leveled(
            false, true, true, true, true, true
        ))
    );
    assert!(details.supports_disabling_thinking());

    // `created` is when OpenRouter listed the model, not a training cutoff, and
    // this entry reports no cutoff of its own.
    assert_eq!(details.knowledge_cutoff, None);
}

/// A reported cutoff is parsed from its `YYYY-MM-DD` form.
#[test]
fn test_map_model_parses_knowledge_cutoff() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "vendor/dated",
        "name": "Dated",
        "created": 1_784_912_546,
        "context_length": 8_192,
        "knowledge_cutoff": "2021-09-30",
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(
        details.knowledge_cutoff,
        chrono::NaiveDate::from_ymd_opt(2021, 9, 30)
    );
}

/// An unparseable cutoff is treated as absent rather than failing the listing.
#[test]
fn test_map_model_ignores_malformed_knowledge_cutoff() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "vendor/bad-date",
        "name": "Bad Date",
        "created": 1_784_912_546,
        "context_length": 8_192,
        "knowledge_cutoff": "September 2021",
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.knowledge_cutoff, None);
}

/// `mandatory` reasoning is the one capability any provider reports that maps
/// onto reasoning being impossible to turn off.
#[test]
fn test_map_model_mandatory_reasoning_is_always_on() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "vendor/always-thinks",
        "name": "Always Thinks",
        "created": 1_784_912_546,
        "context_length": 200_000,
        "supported_parameters": ["reasoning"],
        "reasoning": {
            "mandatory": true,
            "supported_efforts": ["high", "low"],
        },
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert!(
        !details.supports_disabling_thinking(),
        "mandatory reasoning must not be reported as disableable"
    );
}

/// A model advertising no reasoning parameter at all is recorded as not
/// reasoning, rather than left unknown.
#[test]
fn test_map_model_without_reasoning_is_unsupported() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "vendor/plain",
        "name": "Plain",
        "created": 1_784_912_546,
        "context_length": 8_192,
        "supported_parameters": ["max_tokens", "stop"],
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(
        details.reasoning,
        Some(ModelReasoningDetails::unsupported())
    );
    assert_eq!(details.structured_output, Some(false));
    assert_eq!(details.max_output_tokens, None);
}

/// The serving provider's context window wins over the model's own, which may
/// be larger than what a request can actually get.
#[test]
fn test_map_model_prefers_serving_provider_context_window() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "vendor/routed",
        "name": "Routed",
        "created": 1_784_912_546,
        "context_length": 1_000_000,
        "top_provider": {"context_length": 200_000, "max_completion_tokens": 64_000},
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.context_window, Some(200_000));
    assert_eq!(details.max_output_tokens, Some(64_000));
}

/// A provider that advertises no window of its own falls back to the model's.
#[test]
fn test_map_model_falls_back_to_model_context_window() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "vendor/unrouted",
        "name": "Unrouted",
        "created": 1_784_912_546,
        "context_length": 8_192,
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.context_window, Some(8_192));
}

/// A reasoning block naming no efforts describes nothing, so support stays
/// unknown rather than becoming a known ladder with no rungs.
/// Building one would send a `minimal` effort the catalog never announced.
#[test]
fn test_map_model_empty_supported_efforts_stays_unknown() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "vendor/undescribed",
        "name": "Undescribed",
        "created": 1_784_912_546,
        "context_length": 200_000,
        "supported_parameters": ["reasoning", "reasoning_effort"],
        "reasoning": {"mandatory": true, "supported_efforts": []},
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.reasoning, None);
    // Unknown support still reads as unable to disable, so nothing is lost by
    // not preserving `mandatory` as a ladder.
    assert!(!details.supports_disabling_thinking());
}

/// A catalog entry that lists no parameters at all reported nothing, which is
/// not the same as the model supporting nothing.
#[test]
fn test_map_model_without_parameters_leaves_support_unknown() {
    let model: response::Model = serde_json::from_value(serde_json::json!({
        "id": "vendor/bare",
        "name": "Bare",
        "created": 1_784_912_546,
        "context_length": 8_192,
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.structured_output, None);
    assert_eq!(details.reasoning, None);
}

#[test]
fn test_map_models_skips_invalid_catalog_entries() {
    let entry = |id: &str| response::Model {
        id: id.to_owned(),
        name: id.to_owned(),
        created: types::response::OffsetDateTimeFmt(chrono::DateTime::UNIX_EPOCH),
        context_length: 128_000,
        top_provider: types::response::TopProvider::default(),
        supported_parameters: vec![],
        reasoning: None,
        knowledge_cutoff: None,
    };

    // An invalid ID in the live Openrouter catalog must not fail the fetch
    // for unrelated models. The `~`-prefixed latest-alias listings are valid
    // and must be kept.
    let models = map_models(vec![
        entry("z-ai/glm-5.2"),
        entry("~anthropic/claude-fable-latest"),
        entry("model with spaces"),
        entry("anthropic/claude-haiku-4.5"),
    ]);

    assert_eq!(
        models
            .iter()
            .map(|m| m.id.name.as_ref())
            .collect::<Vec<_>>(),
        vec![
            "z-ai/glm-5.2",
            "~anthropic/claude-fable-latest",
            "anthropic/claude-haiku-4.5"
        ]
    );
}

/// The metadata map is keyed by the struct's field names, in field order, and
/// carries only the fields that hold a value.
///
/// Pins the exact shape because these keys are a wire contract: they are
/// persisted in conversation event metadata and read back by the provider that
/// owns each one.
#[test]
fn multi_provider_metadata_maps_present_fields_only() {
    let metadata = MultiProviderMetadata {
        openai_encrypted_content: Some("enc".into()),
        anthropic_thinking_signature: None,
        anthropic_redacted_thinking: Some("redacted".into()),
        google_thought_signature: None,
        openrouter_metadata: vec![Map::from_iter([(
            "field".to_owned(),
            "anthropic_redacted_thinking".into(),
        )])],
    };

    let map = Map::from(metadata);

    assert_eq!(
        serde_json::to_string(&map).unwrap(),
        r#"{"openai_encrypted_content":"enc","anthropic_redacted_thinking":"redacted","openrouter_metadata":[{"field":"anthropic_redacted_thinking"}]}"#
    );
}

/// A metadata value the sub-provider did not report is absent from the map, and
/// an empty `openrouter_metadata` list is omitted rather than emitted as `[]`.
#[test]
fn multi_provider_metadata_omits_absent_fields() {
    let map = Map::from(MultiProviderMetadata::default());

    assert_eq!(serde_json::to_string(&map).unwrap(), "{}");
}

async fn run_test(
    test_name: impl AsRef<str>,
    requests: impl IntoIterator<Item = TestRequest>,
) -> Result {
    crate::test::run_chat_completion(
        test_name,
        env!("CARGO_MANIFEST_DIR"),
        ProviderId::Openrouter,
        LlmProviderConfig::default(),
        requests.into_iter().collect(),
    )
    .await
}
