use std::time::Duration;

use camino_tempfile::tempdir;
use chrono::{TimeZone as _, Utc};
use jp_config::AppConfig;
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId,
    event::{ChatRequest, ChatResponse, ToolCallResponse},
};
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals,
    cmd::{conversation_id::FlagIds, target::ConversationTarget},
};

fn setup_ctx_with_events(
    events: Vec<(ConversationId, Vec<ConversationEvent>)>,
) -> (Ctx, Vec<ConversationId>, SharedBuffer) {
    let tmp = tempdir().unwrap();
    let config = AppConfig::new_test();
    let workspace = Workspace::new(tmp.path());
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut ctx = Ctx::new(
        workspace,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    let mut ids = vec![];
    for (id, evts) in events {
        ctx.workspace
            .create_conversation_with_id(id, Conversation::default(), ctx.config());
        let h = ctx.workspace.acquire_conversation(&id).unwrap();
        let lock = ctx.workspace.test_lock(h);
        lock.as_mut().update_events(|e| e.extend(evts));
        ids.push(id);
    }

    (ctx, ids, out)
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(chrono::DateTime::<Utc>::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap()
}

#[test]
fn test_grep_finds_chat_request() {
    let id = make_id(1000);
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from("tell me about Rust generics"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "generics".into(),
        target: FlagIds::default(),
        ignore_case: false,
    };

    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    assert!(
        output.contains("generics"),
        "expected match in output: {output}"
    );
    assert!(output.contains(&id.to_string()));
}

#[test]
fn test_grep_finds_chat_response() {
    let id = make_id(2000);
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatResponse::message("Rust's type system is powerful"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "type system".into(),
        target: FlagIds::default(),
        ignore_case: false,
    };

    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    assert!(output.contains("type system"));
}

#[test]
fn test_grep_case_insensitive() {
    let id = make_id(3000);
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from("Tell me about WASM"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    // Case-sensitive: no match
    let grep = Grep {
        pattern: "wasm".into(),
        target: FlagIds::default(),
        ignore_case: false,
    };
    assert!(grep.run(&mut ctx, vec![]).is_err());

    // Case-insensitive: match
    let grep = Grep {
        pattern: "wasm".into(),
        target: FlagIds::default(),
        ignore_case: true,
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    assert!(output.contains("WASM"));
}

#[test]
fn test_grep_no_matches() {
    let id = make_id(4000);
    let (mut ctx, _, _out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from("hello"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "nonexistent".into(),
        target: FlagIds::default(),
        ignore_case: false,
    };
    assert!(grep.run(&mut ctx, vec![]).is_err());
}

#[test]
fn test_grep_with_specific_id() {
    let id1 = make_id(5000);
    let id2 = make_id(6000);
    let (mut ctx, _, out) = setup_ctx_with_events(vec![
        (id1, vec![ConversationEvent::new(
            ChatRequest::from("unique-marker-alpha"),
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        )]),
        (id2, vec![ConversationEvent::new(
            ChatRequest::from("unique-marker-beta"),
            Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap(),
        )]),
    ]);

    // Search only in id1
    let grep = Grep {
        pattern: "unique-marker".into(),
        target: FlagIds {
            ids: vec![ConversationTarget::Id(id1)],
        },
        ignore_case: false,
    };
    let h = ctx.workspace.acquire_conversation(&id1).unwrap();
    grep.run(&mut ctx, vec![h]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    assert!(output.contains("alpha"));
    assert!(!output.contains("beta"));
}

#[test]
fn test_grep_searches_tool_call_response() {
    let id = make_id(7000);
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ToolCallResponse {
            id: "tc1".into(),
            result: Ok("file content with secret-keyword here".into()),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "secret-keyword".into(),
        target: FlagIds::default(),
        ignore_case: false,
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    assert!(output.contains("secret-keyword"));
}

#[test]
fn test_grep_multiple_matches_per_conversation() {
    let id = make_id(8000);
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![
        ConversationEvent::new(
            ChatRequest::from("first mention of tokio"),
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        ),
        ConversationEvent::new(
            ChatResponse::message("tokio is an async runtime"),
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 1, 0).unwrap(),
        ),
    ])]);

    let grep = Grep {
        pattern: "tokio".into(),
        target: FlagIds::default(),
        ignore_case: false,
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert_eq!(lines.len(), 2, "expected two matches: {output}");
}

#[test]
fn test_truncate_line_short() {
    assert_eq!(truncate_line("hello", 60), "hello");
}

#[test]
fn test_truncate_line_exact() {
    let s = "a".repeat(60);
    assert_eq!(truncate_line(&s, 60), s);
}

#[test]
fn test_truncate_line_long() {
    let s = "a".repeat(80);
    let result = truncate_line(&s, 60);
    assert!(result.ends_with("..."));
    assert_eq!(result.len(), 63); // 60 + "..."
}

#[test]
fn test_truncate_line_trims_whitespace() {
    assert_eq!(truncate_line("  hello  ", 60), "hello");
}

#[test]
fn test_truncate_line_unicode() {
    // Each emoji is 4 bytes but 1 char — make sure we don't split mid-char
    let s = "🎉".repeat(70);
    let result = truncate_line(&s, 60);
    assert!(result.ends_with("..."));
    // Should truncate at a char boundary
    for c in result.chars() {
        assert!(c == '🎉' || c == '.');
    }
}

#[test]
fn test_event_text_content_chat_request() {
    let kind = EventKind::ChatRequest("hello world".into());
    let texts = event_text_content(&kind);
    assert_eq!(texts, vec!["hello world"]);
}

#[test]
fn test_event_text_content_chat_response_message() {
    let kind = EventKind::ChatResponse(ChatResponse::message("response text"));
    let texts = event_text_content(&kind);
    assert_eq!(texts, vec!["response text"]);
}

#[test]
fn test_event_text_content_chat_response_reasoning() {
    let kind = EventKind::ChatResponse(ChatResponse::reasoning("thinking..."));
    let texts = event_text_content(&kind);
    assert_eq!(texts, vec!["thinking..."]);
}

#[test]
fn test_event_text_content_turn_start() {
    let kind = EventKind::TurnStart(jp_conversation::event::TurnStart);
    let texts = event_text_content(&kind);
    assert!(texts.is_empty());
}
