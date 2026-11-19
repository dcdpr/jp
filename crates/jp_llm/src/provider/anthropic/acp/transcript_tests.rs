use async_anthropic::types::MessageRole;
use camino::Utf8Path;
use datetime_literal::datetime;
use jp_attachment::Attachment;
use jp_config::{AppConfig, PartialAppConfig};
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
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query()).unwrap();
    assert_eq!(prepared.prompt, vec!["What is the code?"]);
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
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
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
fn session_ids_and_timestamps_do_not_change_the_model_visible_prefix() {
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query()).unwrap();
    let first = prepared.records(
        Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        Utf8Path::new("/work/project"),
        datetime!(2026-09-11 12:00:00 Z),
    );
    let second = prepared.records(
        Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        Utf8Path::new("/work/project"),
        datetime!(2026-09-11 12:01:00 Z),
    );
    let content = |records: Vec<NativeRecord<'_>>| {
        records
            .into_iter()
            .map(|record| {
                let value = serde_json::to_value(record).unwrap();
                json!({"role":value["message"]["role"],"content":value["message"]["content"]})
            })
            .collect::<Vec<_>>()
    };
    assert_ne!(first[0].uuid, second[0].uuid);
    assert_eq!(content(first), content(second));
}

#[test]
fn unsupported_parameters_do_not_block_subscription_requests() {
    let mut query = query();
    query.thread.events.add_config_delta(serde_json::from_value::<PartialAppConfig>(json!({"assistant":{"model":{"parameters":{"temperature":0.4,"top_p":0.8,"top_k":10,"service_tier":"flex","custom_option":true}}}})).unwrap());
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.prompt, vec!["What is the code?"]);
}

// Claude Code answers a resumed transcript that ends in a user message by
// inserting an assistant reply of its own, so unanswered requests go into the
// prompt together instead of leaving the earlier one in the transcript.
#[test]
fn consecutive_requests_move_into_the_prompt_together() {
    let mut query = query();
    query.thread.events.extend([ConversationEvent::new(
        ChatRequest::from("One more instruction."),
        datetime!(2026-09-11 12:00:01 Z),
    )]);
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.prompt, vec![
        "What is the code?",
        "One more instruction."
    ]);
    assert_eq!(
        serde_json::to_value(prepared.history.last().unwrap()).unwrap(),
        json!({"role":"assistant","content":[{"type":"text","text":"Found it."}]})
    );
}

// Tool results have no `session/prompt` form, so the transcript has to end in
// them, and the prompt names what Claude Code adds after them.
#[test]
fn trailing_tool_results_prompt_disowns_the_inserted_reply() {
    let timestamp = datetime!(2026-09-11 12:00:00 Z);
    let mut stream =
        ConversationStream::new(AppConfig::new_test().into()).with_created_at(timestamp);
    stream.extend([
        ConversationEvent::new(ChatRequest::from("Find the code."), timestamp),
        ConversationEvent::new(
            ToolCallRequest::new("call_fixed".into(), "lookup".into(), Map::new()),
            timestamp,
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "call_fixed".into(),
                result: Ok("DOGWOOD".into()),
            },
            timestamp,
        ),
    ]);
    let query: ChatQuery = ThreadBuilder::new()
        .with_events(stream)
        .build()
        .unwrap()
        .into();
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.prompt, vec![
        "The client that resumed this session may have added an assistant message reading \"No \
         response requested.\" after the last user message, and the text \"Continue from where \
         you left off.\" to that user message. Neither you nor the user wrote them; disregard \
         both.",
        "Continue your response exactly from where you left off. Do not repeat content you \
         already produced.",
    ]);
    assert_eq!(
        serde_json::to_value(prepared.history.last().unwrap()).unwrap(),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_fixed","content":"DOGWOOD","is_error":false}]})
    );
}

// Attachments share the first user message with the request. They stay in the
// transcript as native documents, which the prompt cannot carry.
#[test]
fn attachments_stay_in_the_transcript_and_the_prompt_disowns_the_inserted_reply() {
    let timestamp = datetime!(2026-09-11 12:00:00 Z);
    let mut stream =
        ConversationStream::new(AppConfig::new_test().into()).with_created_at(timestamp);
    stream.extend([ConversationEvent::new(
        ChatRequest::from("Summarize the notes."),
        timestamp,
    )]);
    let query: ChatQuery = ThreadBuilder::new()
        .with_attachments(vec![Attachment::text("file:///notes.md", "OAK")])
        .with_events(stream)
        .build()
        .unwrap()
        .into();
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.prompt, vec![
        RESUMED_TRANSCRIPT_NOTE,
        "Summarize the notes."
    ]);
    assert_eq!(prepared.history.len(), 1);
    let history = serde_json::to_value(&prepared.history[0]).unwrap();
    assert_eq!(history["role"], "user");
    assert_eq!(history["content"].as_array().unwrap().len(), 1);
    assert_eq!(history["content"][0]["type"], "document");
}

#[test]
fn an_empty_thread_opens_a_new_session_with_the_continue_message() {
    let query: ChatQuery = ThreadBuilder::new()
        .with_events(ConversationStream::new(AppConfig::new_test().into()))
        .build()
        .unwrap()
        .into();
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();
    assert_eq!(prepared.prompt, vec![CONTINUE_MESSAGE]);
    assert!(prepared.history.is_empty());
}
