use jp_config::providers::llm::LlmProviderConfig;
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
