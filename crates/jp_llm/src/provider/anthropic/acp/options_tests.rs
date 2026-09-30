use datetime_literal::datetime;
use jp_config::{AppConfig, model::parameters::ReasoningConfig};
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ConversationEvent},
    thread::ThreadBuilder,
};
use serde_json::json;

use super::*;

/// The `thinking` option Claude Code receives for `model` with `reasoning =
/// off`, built from the model details a subscription query uses.
fn thinking_with_reasoning_off(model: &str) -> Option<Value> {
    let mut config = AppConfig::new_test();
    config.assistant.model.parameters.reasoning = Some(ReasoningConfig::Off);
    let timestamp = datetime!(2026-09-30 12:00:00 Z);
    let mut events = ConversationStream::new(config.into()).with_created_at(timestamp);
    events.extend([ConversationEvent::new(
        ChatRequest::from("Hello."),
        timestamp,
    )]);
    let thread = ThreadBuilder::new().with_events(events).build().unwrap();

    let model = super::super::model_details(&model.parse().unwrap());
    let prepared = PreparedRequest::new(&model, thread.into()).unwrap();
    let metadata = metadata(&prepared, &BTreeMap::new()).unwrap();

    metadata["claudeCode"]["options"].get("thinking").cloned()
}

/// Sonnet 5.5 rejects `thinking: disabled`; `between_tools` is the lowest
/// setting it accepts.
#[test]
fn reasoning_off_on_sonnet_5_5_sends_between_tools() {
    assert_eq!(
        thinking_with_reasoning_off("claude-sonnet-5-5"),
        Some(json!({"type": "between_tools"}))
    );
}

/// Opus 5.5 and Fable reject `thinking: disabled` and have no lower setting, so
/// the option is left out and the model thinks adaptively.
#[test]
fn reasoning_off_on_an_always_on_model_sends_no_thinking() {
    assert_eq!(thinking_with_reasoning_off("claude-opus-5-5"), None);
    assert_eq!(thinking_with_reasoning_off("claude-fable-5-1"), None);
    assert_eq!(thinking_with_reasoning_off("claude-fable-5"), None);
}

/// A model that can turn thinking off, or one this binary does not know, is
/// sent an explicit disable.
#[test]
fn reasoning_off_on_other_models_disables_thinking() {
    assert_eq!(
        thinking_with_reasoning_off("claude-opus-5"),
        Some(json!({"type": "disabled"}))
    );
    assert_eq!(
        thinking_with_reasoning_off("future-model"),
        Some(json!({"type": "disabled"}))
    );
}

/// The camelCase `budgetTokens` is the whole reason `Thinking` exists rather
/// than serializing `ExtendedThinking` directly.
/// Pin it, or the type looks like redundant duplication to the next reader.
#[test]
fn a_thinking_budget_is_spelled_the_way_the_sdk_spells_it() {
    let thinking = Thinking::from(&ExtendedThinking::Enabled {
        budget_tokens: 4096,
        display: Some(ThinkingDisplay::Summarized),
    });

    assert_eq!(
        serde_json::to_value(thinking).unwrap(),
        json!({"type": "enabled", "budgetTokens": 4096, "display": "summarized"})
    );
}

#[test]
fn adaptive_thinking_carries_its_display() {
    let thinking = Thinking::from(&ExtendedThinking::Adaptive {
        display: Some(ThinkingDisplay::Omitted),
    });

    assert_eq!(
        serde_json::to_value(thinking).unwrap(),
        json!({"type": "adaptive", "display": "omitted"})
    );
}

/// An absent `display` is left out rather than sent as `null`, so Claude Code
/// applies its own default instead of rejecting an unexpected type.
#[test]
fn an_unset_display_is_omitted_from_the_payload() {
    let thinking = Thinking::from(&ExtendedThinking::Adaptive { display: None });

    assert_eq!(
        serde_json::to_value(thinking).unwrap(),
        json!({"type": "adaptive"})
    );
}

#[test]
fn disabled_thinking_carries_nothing_else() {
    let thinking = Thinking::from(&ExtendedThinking::Disabled);

    assert_eq!(
        serde_json::to_value(thinking).unwrap(),
        json!({"type": "disabled"})
    );
}

/// `between_tools` rejects every other field, `display` included.
#[test]
fn between_tools_thinking_carries_nothing_else() {
    let thinking = Thinking::from(&ExtendedThinking::BetweenTools);

    assert_eq!(
        serde_json::to_value(thinking).unwrap(),
        json!({"type": "between_tools"})
    );
}
