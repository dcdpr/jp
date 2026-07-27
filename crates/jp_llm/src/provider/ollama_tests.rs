use super::*;

/// Ollama reports the trained context length per model, so it is used rather
/// than left unknown.
/// A model advertising `thinking` keeps its reasoning support unknown, since no
/// effort ladder is reported to derive one from.
#[test]
fn map_model_uses_reported_context_and_capabilities() {
    let model: LocalModel = serde_json::from_value(serde_json::json!({
        "name": "qwen3.5:9b",
        "modified_at": "2026-03-06T03:05:33.981620282+01:00",
        "size": 6_594_474_711_u64,
        "details": {"family": "qwen35", "context_length": 262_144},
        "capabilities": ["vision", "completion", "tools", "thinking"],
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.context_window, Some(262_144));
    // Reports thinking, but no ladder, so support stays unknown.
    assert_eq!(details.reasoning, None);
    // `/api/tags` reports no generation ceiling.
    assert_eq!(details.max_output_tokens, None);
}

/// A model whose reported capabilities omit `thinking` does not reason.
#[test]
fn map_model_without_thinking_capability_is_unsupported() {
    let model: LocalModel = serde_json::from_value(serde_json::json!({
        "name": "llama3:latest",
        "modified_at": "2025-09-16T21:31:42.798881306+02:00",
        "size": 4_661_224_676_u64,
        "details": {"family": "llama", "context_length": 8_192},
        "capabilities": ["completion"],
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.reasoning, Some(ReasoningDetails::unsupported()));
    assert_eq!(details.context_window, Some(8_192));
}

/// An Ollama version that reports no capabilities leaves support unknown,
/// rather than an empty list being read as "this model does not reason".
#[test]
fn map_model_without_reported_capabilities_stays_unknown() {
    let model: LocalModel = serde_json::from_value(serde_json::json!({
        "name": "legacy:7b",
        "modified_at": "",
        "size": 0,
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.reasoning, None);
    assert_eq!(details.context_window, None);
}
