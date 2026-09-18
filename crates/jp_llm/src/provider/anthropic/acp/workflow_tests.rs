use async_anthropic::types::JsonOutputFormat;
use datetime_literal::datetime;
use jp_config::{AppConfig, PartialAppConfig};
use jp_conversation::{
    Compaction, ConversationStream, SummaryPolicy,
    event::{
        ChatRequest, ChatResponse, ConversationEvent, EventKind, ToolCallRequest, ToolCallResponse,
        TurnStart,
    },
    thread::ThreadBuilder,
};
use serde_json::{Map, json};

use super::transcript::PreparedRequest;
use crate::query::ChatQuery;

fn conversation() -> ChatQuery {
    let timestamp = datetime!(2026-09-11 12:00:00 Z);
    let mut stream =
        ConversationStream::new(AppConfig::new_test().into()).with_created_at(timestamp);
    stream.extend([
        ConversationEvent::new(TurnStart, timestamp),
        ConversationEvent::new(ChatRequest::from("Lookup."), timestamp),
        ConversationEvent::new(
            ToolCallRequest::new("call-fixed".into(), "lookup".into(), Map::new()),
            timestamp,
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "call-fixed".into(),
                result: Ok("OAK".into()),
            },
            timestamp,
        ),
        ConversationEvent::new(ChatResponse::message("Found OAK."), timestamp),
    ]);
    stream.add_config_delta(
        serde_json::from_value::<PartialAppConfig>(
            json!({"assistant":{"model":{"id":"openai/gpt-6-astra"}}}),
        )
        .unwrap(),
    );
    stream.extend([
        ConversationEvent::new(TurnStart, timestamp),
        ConversationEvent::new(ChatRequest::from("Check."), timestamp),
        ConversationEvent::new(ChatResponse::reasoning("Foreign reasoning."), timestamp),
        ConversationEvent::new(ChatResponse::message("Confirmed OAK."), timestamp),
    ]);
    stream.add_config_delta(
        serde_json::from_value::<PartialAppConfig>(
            json!({"assistant":{"model":{"id":"anthropic/claude-opus-5"}}}),
        )
        .unwrap(),
    );
    stream.extend([
        ConversationEvent::new(TurnStart, timestamp),
        ConversationEvent::new(ChatRequest::from("Continue."), timestamp),
    ]);
    ThreadBuilder::new()
        .with_system_prompt("Original instructions.")
        .with_events(stream)
        .build()
        .unwrap()
        .into()
}

#[test]
fn returning_to_anthropic_retains_the_intervening_openai_turn() {
    let model = super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, conversation()).unwrap();
    assert_eq!(prepared.prompt, "Continue.");
    assert_eq!(
        serde_json::to_value(prepared.history).unwrap(),
        json!([
            {"role":"user","content":[{"type":"text","text":"Lookup."}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"call-fixed","name":"mcp__jp__lookup","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-fixed","content":"OAK","is_error":false}]},
            {"role":"assistant","content":[{"type":"text","text":"Found OAK."}]},
            {"role":"user","content":[{"type":"text","text":"Check."}]},
            {"role":"assistant","content":[{"type":"text","text":"<think>\nForeign reasoning.\n</think>\n\n"},{"type":"text","text":"Confirmed OAK."}]}
        ])
    );
}

#[test]
fn selected_turn_fork_excludes_unselected_history() {
    let mut query = conversation();
    query.thread.events.retain_turns(|index| index != 1);
    let model = super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.prompt, "Continue.");
    assert_eq!(
        serde_json::to_value(prepared.history).unwrap(),
        json!([
            {"role":"user","content":[{"type":"text","text":"Lookup."}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"call-fixed","name":"mcp__jp__lookup","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-fixed","content":"OAK","is_error":false}]},
            {"role":"assistant","content":[{"type":"text","text":"Found OAK."}]}
        ])
    );
}

#[test]
fn replay_uses_the_replacement_request() {
    let mut query = conversation();
    query.thread.events.retain_turns(|index| index < 2);
    query.thread.events.extend([
        ConversationEvent::new(TurnStart, datetime!(2026-09-11 12:01:00 Z)),
        ConversationEvent::new(
            ChatRequest::from("Revisit instead."),
            datetime!(2026-09-11 12:01:00 Z),
        ),
    ]);
    let model = super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.prompt, "Revisit instead.");
    assert_eq!(prepared.history.len(), 6);
    assert_eq!(
        serde_json::to_value(&prepared.history[5]).unwrap(),
        json!({"role":"assistant","content":[{"type":"text","text":"<think>\nForeign reasoning.\n</think>\n\n"},{"type":"text","text":"Confirmed OAK."}]})
    );
}

#[test]
fn compacted_view_is_encoded_without_mutating_raw_history() {
    let mut query = conversation();
    let mut compaction = Compaction::new(0, 1).with_summary(SummaryPolicy::authored(
        "Lookup and verification established OAK.",
    ));
    compaction.timestamp = datetime!(2026-09-11 12:01:00 Z);
    query.thread.events.add_compaction(compaction);
    let before = query
        .thread
        .events
        .iter()
        .map(|event| event.event.clone())
        .collect::<Vec<_>>();
    let model = super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query.clone()).unwrap();
    assert_eq!(prepared.prompt, "Continue.");
    assert_eq!(prepared.history.len(), 2);
    insta::assert_json_snapshot!("compacted_native_history", prepared.history);
    assert_eq!(
        query
            .thread
            .events
            .iter()
            .map(|event| event.event.clone())
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn tool_result_continuation_does_not_repeat_the_previous_request() {
    let mut query = conversation();
    query.thread.events.retain_turns(|index| index == 0);
    query
        .thread
        .events
        .retain(|event| !matches!(event.kind, EventKind::ChatResponse(_)));
    let model = super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(
        prepared.prompt,
        "Continue your response exactly from where you left off. Do not repeat content you \
         already produced."
    );
    assert_eq!(prepared.history.len(), 3);
    assert_eq!(
        serde_json::to_value(&prepared.history[2]).unwrap(),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call-fixed","content":"OAK","is_error":false}]})
    );
}

#[test]
fn large_historical_tool_results_remain_complete() {
    let mut query = conversation();
    let content = "0123456789abcdef".repeat(15_000) + "END-OF-RESULT";
    for event in query.thread.events.iter_mut() {
        if let EventKind::ToolCallResponse(response) = &mut event.event.kind {
            response.result = Ok(content.clone());
        }
    }
    let model = super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    let message = serde_json::to_value(&prepared.history[2]).unwrap();
    // This is a preservation assertion, not a generated-output expectation.
    assert_eq!(
        message["content"][0]["content"].as_str(),
        Some(content.as_str())
    );
    assert_eq!(prepared.prompt, "Continue.");
}

#[test]
fn changed_instructions_schema_and_tool_result_reach_the_next_request() {
    let mut query = conversation();
    query.thread.system_prompt = Some("Replacement instructions.".into());
    let schema = json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}).as_object().unwrap().clone();
    for event in query.thread.events.iter_mut() {
        if let EventKind::ToolCallResponse(response) = &mut event.event.kind {
            response.result = Ok("CEDAR".into());
        }
    }
    if let EventKind::ChatRequest(request) =
        &mut query.thread.events.iter_mut().last().unwrap().event.kind
    {
        request.schema = Some(schema.clone());
    } else {
        panic!("expected the pending request")
    }
    let model = super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.system_prompt, "Replacement instructions.");
    assert_eq!(
        prepared.schema,
        Some(JsonOutputFormat::JsonSchema { schema })
    );
    assert_eq!(
        serde_json::to_value(&prepared.history[2]).unwrap(),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call-fixed","content":"CEDAR","is_error":false}]})
    );
}
