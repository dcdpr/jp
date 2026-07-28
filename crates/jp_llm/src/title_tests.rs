use std::sync::{Arc, Mutex};

use jp_config::model::{
    id::{ModelIdConfig, Name, ProviderId},
    parameters::ReasoningConfig,
};

use super::*;
use crate::{
    event::{Event, FinishReason},
    provider::mock::MockProvider,
};

fn model_id(name: &str) -> ModelIdConfig {
    ModelIdConfig {
        provider: ProviderId::Test,
        name: name.parse().expect("valid model name"),
    }
}

fn model_config(name: &str) -> ModelConfig {
    ModelConfig {
        id: ModelIdOrAliasConfig::Id(model_id(name)),
        parameters: ParametersConfig::default(),
    }
}

/// A provider that answers with one title, plus the log of what it received.
fn title_provider(context_window: Option<u32>) -> (MockProvider, Arc<Mutex<Vec<ChatQuery>>>) {
    MockProvider::new(vec![
        Event::structured(0, json!({"titles": ["A Title"]}).to_string()),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ])
    .with_model(ModelDetails {
        context_window,
        ..ModelDetails::empty(model_id("mock"))
    })
    .capturing_requests()
}

async fn details(provider: &MockProvider) -> ModelDetails {
    provider
        .model_details(&"mock".parse::<Name>().unwrap())
        .await
        .unwrap()
}

/// A conversation of `turns` turns, each roughly 1000 chars.
fn long_conversation(turns: usize) -> ConversationStream {
    let mut events = ConversationStream::new_test();
    for i in 0..turns {
        events = events.with_turn(format!("turn {i}: {}", "x".repeat(1000)));
    }
    events
}

/// The reported failure: a conversation grown on a large-window model, titled
/// by a model with a small one.
///
/// The assertion is on the size of what was sent rather than on a turn count:
/// fitting shrinks to a target fraction of the window, and which turn the
/// cutoff lands on shifts with the size of the instruction sections.
/// Without fitting this request carries ~200k chars into a 3k char window.
#[tokio::test]
async fn generate_shrinks_a_conversation_past_the_window() {
    const WINDOW: u32 = 1000;

    let (provider, requests) = title_provider(Some(WINDOW));
    let details = details(&provider).await;
    let conversation = long_conversation(200);
    let original_chars = window::estimate_chars(&conversation);

    let titles = title_generate(&provider, &details, conversation).await;
    assert_eq!(titles, ["A Title"]);

    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);

    let sent_chars = window::estimate_chars(&sent[0].thread.events);
    let window_chars = WINDOW as usize * window::CHARS_PER_TOKEN;
    assert!(
        original_chars > window_chars * 50,
        "fixture must be far past the window, was {original_chars} chars"
    );
    assert!(
        sent_chars < window_chars,
        "sent {sent_chars} chars into a {window_chars} char window"
    );
}

#[tokio::test]
async fn generate_keeps_a_conversation_that_fits() {
    let (provider, requests) = title_provider(Some(1_000_000));
    let details = details(&provider).await;

    title_generate(&provider, &details, long_conversation(3)).await;

    // The three original turns plus the appended title request.
    let sent = requests.lock().unwrap();
    assert_eq!(sent[0].thread.events.turn_count(), 4);
}

/// Providers that don't report a window (local llama.cpp, Ollama) leave the
/// conversation untouched: there is no budget to measure against.
#[tokio::test]
async fn generate_keeps_everything_when_the_window_is_unknown() {
    let (provider, requests) = title_provider(None);
    let details = details(&provider).await;

    title_generate(&provider, &details, long_conversation(200)).await;

    let sent = requests.lock().unwrap();
    assert_eq!(sent[0].thread.events.turn_count(), 201);
}

async fn title_generate(
    provider: &MockProvider,
    details: &ModelDetails,
    events: ConversationStream,
) -> Vec<String> {
    generate(provider, details, TitleRequest {
        events,
        model: model_config("mock"),
        count: 1,
        rejected: vec![],
    })
    .await
    .expect("title generation succeeds")
}

#[test]
fn resolve_model_falls_back_to_the_assistant_model() {
    let mut config = AppConfig::new_test();
    config.assistant.model.id = ModelIdOrAliasConfig::Id(model_id("big-model"));
    config.conversation.title.generate.model = None;

    let model = resolve_model(&config, None);
    assert_eq!(model.id.resolved(), &model_id("big-model"));
}

#[test]
fn resolve_model_prefers_the_configured_title_model() {
    let mut config = AppConfig::new_test();
    config.assistant.model.id = ModelIdOrAliasConfig::Id(model_id("big-model"));
    config.conversation.title.generate.model = Some(model_config("cheap-model"));

    let model = resolve_model(&config, None);
    assert_eq!(model.id.resolved(), &model_id("cheap-model"));
}

#[test]
fn resolve_model_override_outranks_the_configured_title_model() {
    let mut config = AppConfig::new_test();
    config.conversation.title.generate.model = Some(model_config("cheap-model"));

    let override_id = ModelIdOrAliasConfig::Id(model_id("chosen-model"));
    let model = resolve_model(&config, Some(&override_id));
    assert_eq!(model.id.resolved(), &model_id("chosen-model"));
}

/// A title is a short factual summary; without an explicit setting it must not
/// inherit the conversation's reasoning budget.
#[test]
fn resolve_model_defaults_reasoning_to_low_effort() {
    let mut config = AppConfig::new_test();
    config.assistant.model.parameters.reasoning =
        Some(ReasoningConfig::Custom(CustomReasoningConfig {
            effort: ReasoningEffort::Max,
            exclude: false,
        }));

    let model = resolve_model(&config, None);
    assert_eq!(
        model.parameters.reasoning,
        Some(ReasoningConfig::Custom(CustomReasoningConfig {
            effort: ReasoningEffort::Low,
            exclude: true,
        }))
    );
}

#[test]
fn resolve_model_keeps_an_explicit_reasoning_setting() {
    let mut config = AppConfig::new_test();
    let mut title_model = model_config("cheap-model");
    title_model.parameters.reasoning = Some(ReasoningConfig::Off);
    config.conversation.title.generate.model = Some(title_model);

    let model = resolve_model(&config, None);
    assert_eq!(model.parameters.reasoning, Some(ReasoningConfig::Off));
}

#[test]
fn title_schema_has_correct_structure() {
    let schema = title_schema(3);

    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["titles"]["type"], "array");
    assert_eq!(schema["properties"]["titles"]["minItems"], 3);
    assert_eq!(schema["properties"]["titles"]["maxItems"], 3);
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("titles"))
    );
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn title_schema_single_title() {
    let schema = title_schema(1);
    assert_eq!(schema["properties"]["titles"]["minItems"], 1);
    assert_eq!(schema["properties"]["titles"]["maxItems"], 1);
}

#[test]
fn title_instructions_without_rejected() {
    let sections = title_instructions(3, &[]);
    assert_eq!(sections.len(), 1);
}

#[test]
fn title_instructions_with_rejected() {
    let rejected = vec!["Bad Title".to_owned(), "Worse Title".to_owned()];
    let sections = title_instructions(3, &rejected);
    assert_eq!(sections.len(), 2);
}

#[test]
fn extract_titles_valid() {
    let data = json!({"titles": ["Title A", "Title B"]});
    assert_eq!(extract_titles(&data), vec!["Title A", "Title B"]);
}

#[test]
fn extract_titles_missing_key() {
    let data = json!({"other": "value"});
    assert!(extract_titles(&data).is_empty());
}

#[test]
fn extract_titles_wrong_type() {
    let data = json!({"titles": "not an array"});
    assert!(extract_titles(&data).is_empty());
}

#[test]
fn extract_titles_mixed_types_filters_non_strings() {
    let data = json!({"titles": ["Valid", 42, null, "Also Valid"]});
    assert_eq!(extract_titles(&data), vec!["Valid", "Also Valid"]);
}
