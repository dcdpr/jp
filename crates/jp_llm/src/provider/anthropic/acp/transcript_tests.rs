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
use crate::provider::anthropic::acp::context::{Environment, ModelIdentity};

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
    let records = prepared
        .records(
            session,
            Utf8Path::new("/work/project"),
            datetime!(2026-09-11 12:00:00 Z),
            None,
        )
        .unwrap();
    assert_eq!(records[0]["parentUuid"], Value::Null);
    assert_eq!(records[1]["parentUuid"], records[0]["uuid"]);
    assert_eq!(records[3]["sessionId"], json!(session));
    assert_eq!(records[1]["type"], "assistant");
    insta::assert_json_snapshot!("native_transcript", &records);
    assert_eq!(
        records[1]["message"]["content"],
        json!([
            {"type":"tool_use","id":"call_fixed","name":"mcp__jp__lookup","input":{"project":"compiler"}}
        ])
    );
}

#[test]
fn session_ids_and_timestamps_do_not_change_the_model_visible_prefix() {
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query()).unwrap();
    let first = prepared
        .records(
            Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            Utf8Path::new("/work/project"),
            datetime!(2026-09-11 12:00:00 Z),
            None,
        )
        .unwrap();
    let second = prepared
        .records(
            Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
            Utf8Path::new("/work/project"),
            datetime!(2026-09-11 12:01:00 Z),
            None,
        )
        .unwrap();
    let content = |records: Vec<Value>| {
        records
            .into_iter()
            .map(|value| {
                json!({"role":value["message"]["role"],"content":value["message"]["content"]})
            })
            .collect::<Vec<_>>()
    };
    assert_ne!(first[0]["uuid"], second[0]["uuid"]);
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

// Tool results have no `session/prompt` form, so they remain at the end of the
// transcript and the prompt is the continue message.
#[test]
fn trailing_tool_results_stay_in_the_transcript() {
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
        "Continue your response exactly from where you left off. Do not repeat content you \
         already produced."
    ]);
    assert_eq!(
        serde_json::to_value(prepared.history.last().unwrap()).unwrap(),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_fixed","content":"DOGWOOD","is_error":false}]})
    );
}

// Document attachments share the first user message with the request text.
// The documents remain in the transcript as native `document` blocks, keeping
// their title and context, while the request text moves into the prompt.
#[test]
fn attachments_stay_in_the_transcript() {
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
    assert_eq!(prepared.prompt, vec!["Summarize the notes."]);
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
    assert_eq!(prepared.history, Vec::<Message>::new());
    assert_eq!(prepared.started, None);
}

// The `date` context record keeps the conversation's first day, so it is
// taken from the first request, not the latest.
#[test]
fn a_conversation_starts_at_its_first_request() {
    let first = datetime!(2026-09-11 23:30:00 Z);
    let later = datetime!(2026-09-14 09:00:00 Z);
    let mut stream = ConversationStream::new(AppConfig::new_test().into()).with_created_at(first);
    stream.extend([
        ConversationEvent::new(ChatRequest::from("Hello."), first),
        ConversationEvent::new(ChatResponse::message("Hi."), first),
        ConversationEvent::new(ChatRequest::from("Again."), later),
    ]);
    let query: ChatQuery = ThreadBuilder::new()
        .with_events(stream)
        .build()
        .unwrap()
        .into();
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());

    let prepared = PreparedRequest::new(&model, query).unwrap();

    assert_eq!(prepared.started, Some(first));
}

/// A conversation with notes attached, whose requests are `requests` in order
/// with `answers` after all but the last.
fn notes_conversation(requests: &[&str], answers: &[&str]) -> PreparedRequest {
    let timestamp = datetime!(2026-09-11 12:00:00 Z);
    let mut stream =
        ConversationStream::new(AppConfig::new_test().into()).with_created_at(timestamp);
    for (index, request) in requests.iter().enumerate() {
        stream.extend([ConversationEvent::new(
            ChatRequest::from(*request),
            timestamp,
        )]);
        if let Some(answer) = answers.get(index) {
            stream.extend([ConversationEvent::new(
                ChatResponse::message(*answer),
                timestamp,
            )]);
        }
    }
    let query: ChatQuery = ThreadBuilder::new()
        .with_attachments(vec![Attachment::text("file:///notes.md", "OAK")])
        .with_events(stream)
        .build()
        .unwrap()
        .into();
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    PreparedRequest::new(&model, query).unwrap()
}

const SESSION: &str = "22222222-2222-4222-8222-222222222222";

fn session_context() -> SessionContext {
    SessionContext {
        environment: Environment {
            working_directory: "/work/project".into(),
            is_worktree: false,
            is_git_repo: true,
            platform: "darwin",
            shell: "zsh (login shell: fish)".into(),
            os_version: "Darwin 27.0.0".into(),
        },
        model: ModelIdentity {
            model_id: "claude-sonnet-5-5".into(),
            marketing_name: "Sonnet 5.5".into(),
            knowledge_cutoff: "June 2026",
        },
        user_email: None,
        date: "2026-09-11".into(),
        organization_uuid: Some("00000000-0000-4000-8000-000000000000".into()),
    }
}

/// Each record's type, model, and text or block types, in chain order.
fn shape(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .map(|record| {
            json!({
                "type": record["type"],
                "model": record["message"]["model"],
                "content": record["message"]["content"].as_array().map(|blocks| blocks
                    .iter()
                    .map(|block| block["text"].as_str().or(block["type"].as_str()))
                    .collect::<Vec<_>>()),
                "attachment": record["attachment"]["type"],
            })
        })
        .collect()
}

fn records(prepared: &PreparedRequest, context: Option<&SessionContext>) -> Vec<Value> {
    prepared
        .records(
            Uuid::parse_str(SESSION).unwrap(),
            Utf8Path::new("/work/project"),
            datetime!(2026-09-11 12:01:00 Z),
            context,
        )
        .unwrap()
}

// The first turn's request, as Claude Code built it: the documents, its own
// reply, the prompt, and the context records it added after the prompt.
#[test]
fn the_next_turn_repeats_what_claude_code_sent_on_the_first() {
    let second = notes_conversation(&["Summarize the notes.", "Thanks."], &["OAK."]);
    let context = session_context();

    let records = records(&second, Some(&context));

    assert_eq!(shape(&records), vec![
        json!({"type":"user","model":null,"content":["document"],"attachment":null}),
        json!({"type":"assistant","model":"<synthetic>","content":["No response requested."],"attachment":null}),
        json!({"type":"user","model":null,"content":["Summarize the notes."],"attachment":null}),
        json!({"type":"attachment","model":null,"content":null,"attachment":"environment"}),
        json!({"type":"attachment","model":null,"content":null,"attachment":"model"}),
        json!({"type":"attachment","model":null,"content":null,"attachment":"session_context"}),
        json!({"type":"attachment","model":null,"content":null,"attachment":"date"}),
        json!({"type":"attachment","model":null,"content":null,"attachment":"credential_org"}),
        json!({"type":"assistant","model":"claude-opus-5","content":["OAK."],"attachment":null}),
    ]);
    assert_eq!(records[0]["parentUuid"], Value::Null);
    for pair in records.windows(2) {
        assert_eq!(pair[1]["parentUuid"], pair[0]["uuid"]);
    }
    for record in &records {
        assert_eq!(record["sessionId"], SESSION);
    }
}

// Only the session's first prompt gets context records; Claude Code adds none
// to a prompt whose history already holds them.
#[test]
fn later_prompts_carry_no_context_records() {
    let third = notes_conversation(&["Summarize the notes.", "Thanks.", "More."], &[
        "OAK.", "Sure.",
    ]);
    let context = session_context();

    let records = records(&third, Some(&context));

    let attachments = records
        .iter()
        .filter(|record| record["type"] == "attachment")
        .count();
    assert_eq!(attachments, 5);
    assert_eq!(shape(&records[9..]), vec![
        json!({"type":"user","model":null,"content":["Thanks."],"attachment":null}),
        json!({"type":"assistant","model":"claude-opus-5","content":["Sure."],"attachment":null}),
    ]);
}

// A new turn after one that ended on tool results: Claude Code resumed a
// transcript ending in the results and inserted its reply after them.
#[test]
fn a_prompt_after_tool_results_follows_claude_codes_reply() {
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
        ConversationEvent::new(ChatRequest::from("Try again."), timestamp),
        ConversationEvent::new(ChatResponse::message("DOGWOOD."), timestamp),
        ConversationEvent::new(ChatRequest::from("Thanks."), timestamp),
    ]);
    let query: ChatQuery = ThreadBuilder::new()
        .with_events(stream)
        .build()
        .unwrap()
        .into();
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query).unwrap();

    assert_eq!(shape(&records(&prepared, None)), vec![
        json!({"type":"user","model":null,"content":["Find the code."],"attachment":null}),
        json!({"type":"assistant","model":"claude-opus-5","content":["tool_use"],"attachment":null}),
        json!({"type":"user","model":null,"content":["tool_result"],"attachment":null}),
        json!({"type":"assistant","model":"<synthetic>","content":["No response requested."],"attachment":null}),
        json!({"type":"user","model":null,"content":["Try again."],"attachment":null}),
        json!({"type":"assistant","model":"claude-opus-5","content":["DOGWOOD."],"attachment":null}),
    ]);
}

#[test]
fn the_inserted_reply_is_recorded_as_claude_code_records_it() {
    let second = notes_conversation(&["Summarize the notes.", "Thanks."], &["OAK."]);

    let records = records(&second, None);

    let mut reply = records[1].clone();
    for key in ["uuid", "parentUuid"] {
        reply.as_object_mut().unwrap().remove(key);
    }
    reply["message"].as_object_mut().unwrap().remove("id");
    assert_eq!(
        reply,
        json!({
            "type": "assistant",
            "sessionId": SESSION,
            "cwd": "/work/project",
            "isSidechain": false,
            "isApiErrorMessage": false,
            "timestamp": "2026-09-11T12:01:00Z",
            "message": {
                "type": "message",
                "role": "assistant",
                "model": "<synthetic>",
                "content": [{"type": "text", "text": "No response requested."}],
                "stop_reason": "stop_sequence",
                "stop_sequence": "",
                "usage": {"input_tokens": 0, "output_tokens": 0},
            },
        })
    );
}
