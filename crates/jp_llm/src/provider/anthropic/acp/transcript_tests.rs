use async_anthropic::types::{MessageContent, MessageRole};
use camino::Utf8Path;
use datetime_literal::datetime;
use jp_config::AppConfig;
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ChatResponse, ConversationEvent, ToolCallRequest, ToolCallResponse},
    thread::ThreadBuilder,
};
use serde_json::{Map, json};
use uuid::Uuid;

use super::*;

fn query() -> ChatQuery {
    let timestamp = datetime!(2026-09-11 12:00:00 Z);
    let mut stream =
        ConversationStream::new(AppConfig::new_test().into()).with_created_at(timestamp);
    stream.extend([
        ConversationEvent::new(ChatRequest::from("Find the code."), timestamp),
        ConversationEvent::new(
            ToolCallRequest::new(
                "call_fixed".into(),
                "lookup".into(),
                Map::from_iter([("project".into(), "compiler".into())]),
            ),
            timestamp,
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "call_fixed".into(),
                result: Ok("DOGWOOD".into()),
            },
            timestamp,
        ),
        ConversationEvent::new(ChatResponse::message("Found it."), timestamp),
        ConversationEvent::new(ChatRequest::from("What is the code?"), timestamp),
    ]);
    ThreadBuilder::new()
        .with_system_prompt("Use the supplied history.")
        .with_events(stream)
        .build()
        .unwrap()
        .into()
}

#[test]
fn thread_prefix_retains_roles_and_tool_pairing() {
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap()).unwrap();
    let prepared = PreparedRequest::new(&model, query()).unwrap();
    assert_eq!(prepared.prompt, "What is the code?");
    assert_eq!(prepared.system_prompt, "Use the supplied history.");
    assert_eq!(
        serde_json::to_value(&prepared.history).unwrap(),
        json!([
            {"role":"user","content":[{"type":"text","text":"Find the code."}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_fixed","name":"mcp__jp__lookup","input":{"project":"compiler"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_fixed","content":"DOGWOOD","is_error":false}]},
            {"role":"assistant","content":[{"type":"text","text":"Found it."}]}
        ])
    );
}

#[test]
fn native_bookkeeping_does_not_rewrite_message_content() {
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap()).unwrap();
    let prepared = PreparedRequest::new(&model, query()).unwrap();
    let session = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
    let records = prepared.records(
        session,
        Utf8Path::new("/work/project"),
        datetime!(2026-09-11 12:00:00 Z),
    );
    assert_eq!(records[0].parent_uuid, None);
    assert_eq!(records[1].parent_uuid, Some(records[0].uuid));
    assert_eq!(records[3].session_id, session);
    assert_eq!(records[1].type_, MessageRole::Assistant);
    insta::assert_json_snapshot!("native_transcript", &records);
    assert_eq!(
        serde_json::to_value(&records[1].message).unwrap()["content"],
        json!([
            {"type":"tool_use","id":"call_fixed","name":"mcp__jp__lookup","input":{"project":"compiler"}}
        ])
    );
}

#[test]
fn pending_text_is_removed_without_removing_prior_user_blocks() {
    let mut query = query();
    query.thread.events.extend([ConversationEvent::new(
        ChatRequest::from("One more instruction."),
        datetime!(2026-09-11 12:00:01 Z),
    )]);
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap()).unwrap();
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.prompt, "One more instruction.");
    let last = prepared.history.last().unwrap();
    assert_eq!(last.role, MessageRole::User);
    assert_eq!(last.content.0, vec![MessageContent::Text(
        "What is the code?".into()
    )]);
}
