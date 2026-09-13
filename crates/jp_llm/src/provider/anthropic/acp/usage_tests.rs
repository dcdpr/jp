use async_anthropic::types::CreateMessagesResponse;
use serde_json::json;

use super::*;

#[test]
fn repeated_message_ids_replace_snapshots_instead_of_adding_tokens() {
    let first: CreateMessagesResponse = serde_json::from_value(json!({"id":"msg-fixed","model":"claude-opus-5","usage":{"input_tokens":12,"output_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":200}})).unwrap();
    let later: CreateMessagesResponse = serde_json::from_value(
        json!({"id":"msg-fixed","model":"claude-opus-5","usage":{"output_tokens":7}}),
    )
    .unwrap();
    let mut ledger = UsageLedger::default();
    ledger.observe(&first);
    ledger.observe(&later);
    assert_eq!(
        ledger.snapshot("session-fixed"),
        json!({
            "native_session_id":"session-fixed",
            "requests":{"msg-fixed":{"model":"claude-opus-5","input_tokens":12,"output_tokens":7,"cache_creation_input_tokens":100,"cache_read_input_tokens":200}}
        })
    );
}

#[test]
fn runtime_totals_are_separate_from_main_request_usage() {
    let message: CreateMessagesResponse = serde_json::from_value(json!({"id":"msg-main","model":"claude-opus-5","usage":{"input_tokens":2,"output_tokens":4,"cache_creation_input_tokens":0,"cache_read_input_tokens":500}})).unwrap();
    let mut ledger = UsageLedger::default();
    ledger.observe(&message);
    ledger.set_runtime(serde_json::from_value(json!({
        "usage":{"input_tokens":9,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":500},
        "model_usage":{"claude-opus-5":{"inputTokens":2,"outputTokens":4},"claude-haiku-4-5":{"inputTokens":7,"outputTokens":6}},
        "estimated_cost_usd":0.02
    })).unwrap());
    let snapshot = ledger.snapshot("session-fixed");
    assert_eq!(snapshot["requests"]["msg-main"]["input_tokens"], 2);
    assert_eq!(snapshot["runtime"]["usage"]["input_tokens"], 9);
    assert_eq!(
        snapshot["runtime"]["model_usage"]["claude-haiku-4-5"]["outputTokens"],
        6
    );
    assert_eq!(snapshot["runtime"]["estimated_cost_usd"], 0.02);
}

#[test]
fn missing_usage_is_not_reported_as_zero() {
    let message: CreateMessagesResponse =
        serde_json::from_value(json!({"id":"msg-fixed","model":"claude-opus-5"})).unwrap();
    let mut ledger = UsageLedger::default();
    ledger.observe(&message);
    assert_eq!(
        ledger.snapshot("session-fixed"),
        json!({"native_session_id":"session-fixed","requests":{}})
    );
}
