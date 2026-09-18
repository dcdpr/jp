use serde_json::json;

use super::*;

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
