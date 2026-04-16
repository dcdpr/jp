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
    setup_ctx_with_conversations(
        events
            .into_iter()
            .map(|(id, evts)| (id, Conversation::default(), evts))
            .collect(),
    )
}

fn setup_ctx_with_conversations(
    entries: Vec<(ConversationId, Conversation, Vec<ConversationEvent>)>,
) -> (Ctx, Vec<ConversationId>, SharedBuffer) {
    let tmp = tempdir().unwrap();
    let config = AppConfig::new_test();
    let workspace = Workspace::new(tmp.path());
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut ctx = Ctx::new(
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    let mut ids = vec![];
    for (id, conversation, evts) in entries {
        ctx.workspace
            .create_conversation_with_id(id, conversation, ctx.config());
        let h = ctx.workspace.acquire_conversation(&id).unwrap();
        let lock = ctx.workspace.test_lock(h);
        lock.as_mut().update_events(|e| e.extend(evts));
        ids.push(id);
    }

    (ctx, ids, out)
}

fn setup_ctx_with_json(
    events: Vec<(ConversationId, Vec<ConversationEvent>)>,
) -> (Ctx, Vec<ConversationId>, SharedBuffer) {
    setup_ctx_with_format(events, OutputFormat::Json)
}

fn setup_ctx_with_plain(
    events: Vec<(ConversationId, Vec<ConversationEvent>)>,
) -> (Ctx, Vec<ConversationId>, SharedBuffer) {
    setup_ctx_with_format(events, OutputFormat::Text)
}

fn setup_ctx_with_format(
    events: Vec<(ConversationId, Vec<ConversationEvent>)>,
    format: OutputFormat,
) -> (Ctx, Vec<ConversationId>, SharedBuffer) {
    let entries = events
        .into_iter()
        .map(|(id, evts)| (id, Conversation::default(), evts))
        .collect();
    setup_ctx_with_conversations_and_format(entries, format)
}

fn setup_ctx_with_conversations_and_format(
    entries: Vec<(ConversationId, Conversation, Vec<ConversationEvent>)>,
    format: OutputFormat,
) -> (Ctx, Vec<ConversationId>, SharedBuffer) {
    let tmp = tempdir().unwrap();
    let config = AppConfig::new_test();
    let workspace = Workspace::new(tmp.path());
    let (printer, out, _err) = Printer::memory(format);
    let mut ctx = Ctx::new(
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    let mut ids = vec![];
    for (id, conversation, evts) in entries {
        ctx.workspace
            .create_conversation_with_id(id, conversation, ctx.config());
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
        ..Default::default()
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
        ..Default::default()
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
        ignore_case: false,
        ..Default::default()
    };
    assert!(grep.run(&mut ctx, vec![]).is_err());

    // Case-insensitive: match
    let grep = Grep {
        pattern: "wasm".into(),
        ignore_case: true,
        ..Default::default()
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
        ..Default::default()
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
        target: FlagIds::from_targets(vec![ConversationTarget::Id(id1)]),
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
fn test_context_lines_around_match() {
    let id = make_id(9000);
    let multiline = "line-one\nline-two\nMATCH-here\nline-four\nline-five";
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from(multiline),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        context: 1,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    // context=1 around index 2: lines 1,2,3
    assert_eq!(
        lines.len(),
        3,
        "expected 3 lines (1 context above, match, 1 context below): {output}"
    );
    assert!(lines[0].contains("line-two"));
    assert!(lines[1].contains("MATCH-here"));
    assert!(lines[2].contains("line-four"));
}

#[test]
fn test_context_separator_between_groups() {
    let id = make_id(9100);
    // Matches at lines 0 and 4 with context=1 => two separate groups
    let multiline = "MATCH-alpha\nline-b\nline-c\nline-d\nMATCH-beta";
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from(multiline),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        context: 1,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    assert!(
        output.contains("--"),
        "expected group separator in output: {output}"
    );
}

#[test]
fn test_context_merges_overlapping_ranges() {
    let id = make_id(9200);
    // Matches at lines 1 and 3 with context=1 => merged into single group 0..=4
    let multiline = "line-a\nMATCH-one\nline-c\nMATCH-two\nline-e";
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from(multiline),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        context: 1,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    // All 5 lines merged into one group, no separator
    assert_eq!(lines.len(), 5, "expected all 5 lines merged: {output}");
    assert!(
        !output.contains("--"),
        "should not contain separator: {output}"
    );
}

#[test]
fn test_context_clamps_at_boundaries() {
    let id = make_id(9300);
    // Match at first line with context=3 => should not panic, just clamp
    let multiline = "MATCH-first\nline-b\nline-c";
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from(multiline),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        context: 3,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert_eq!(lines.len(), 3, "expected all 3 lines: {output}");
}

#[test]
fn test_context_zero_unchanged_behavior() {
    let id = make_id(9400);
    let multiline = "line-a\nMATCH-here\nline-c";
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from(multiline),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        context: 0,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "context=0 should show only the match: {output}"
    );
    assert!(lines[0].contains("MATCH-here"));
}

#[test]
fn test_sort_by_created_ascending() {
    // Create conversations in non-chronological order: 3000, 1000, 2000
    let id_a = make_id(3000);
    let id_b = make_id(1000);
    let id_c = make_id(2000);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let (mut ctx, _, out) = setup_ctx_with_events(vec![
        (id_a, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-a"),
            ts,
        )]),
        (id_b, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-b"),
            ts,
        )]),
        (id_c, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-c"),
            ts,
        )]),
    ]);

    let grep = Grep {
        pattern: "MATCH".into(),
        sort: Sort::Created,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert_eq!(lines.len(), 3);
    // Ascending by creation: 1000 < 2000 < 3000
    assert!(
        lines[0].contains("MATCH-b"),
        "first should be id_b (1000): {output}"
    );
    assert!(
        lines[1].contains("MATCH-c"),
        "second should be id_c (2000): {output}"
    );
    assert!(
        lines[2].contains("MATCH-a"),
        "third should be id_a (3000): {output}"
    );
}

#[test]
fn test_sort_by_created_descending() {
    let id_a = make_id(3000);
    let id_b = make_id(1000);
    let id_c = make_id(2000);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let (mut ctx, _, out) = setup_ctx_with_events(vec![
        (id_a, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-a"),
            ts,
        )]),
        (id_b, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-b"),
            ts,
        )]),
        (id_c, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-c"),
            ts,
        )]),
    ]);

    let grep = Grep {
        pattern: "MATCH".into(),
        sort: Sort::Created,
        descending: true,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert_eq!(lines.len(), 3);
    // Descending by creation: 3000 > 2000 > 1000
    assert!(
        lines[0].contains("MATCH-a"),
        "first should be id_a (3000): {output}"
    );
    assert!(
        lines[1].contains("MATCH-c"),
        "second should be id_c (2000): {output}"
    );
    assert!(
        lines[2].contains("MATCH-b"),
        "third should be id_b (1000): {output}"
    );
}

#[test]
fn test_sort_by_activated() {
    let id_a = make_id(1000);
    let id_b = make_id(2000);
    let id_c = make_id(3000);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

    // Activated order: id_b (earliest) < id_c < id_a (latest)
    let conv_a = Conversation::default()
        .with_last_activated_at(Utc.with_ymd_and_hms(2025, 3, 1, 0, 0, 0).unwrap());
    let conv_b = Conversation::default()
        .with_last_activated_at(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    let conv_c = Conversation::default()
        .with_last_activated_at(Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap());

    let (mut ctx, _, out) = setup_ctx_with_conversations(vec![
        (id_a, conv_a, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-a"),
            ts,
        )]),
        (id_b, conv_b, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-b"),
            ts,
        )]),
        (id_c, conv_c, vec![ConversationEvent::new(
            ChatRequest::from("MATCH-c"),
            ts,
        )]),
    ]);

    let grep = Grep {
        pattern: "MATCH".into(),
        sort: Sort::Activated,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(
        lines[0].contains("MATCH-b"),
        "first should be id_b (Jan): {output}"
    );
    assert!(
        lines[1].contains("MATCH-c"),
        "second should be id_c (Feb): {output}"
    );
    assert!(
        lines[2].contains("MATCH-a"),
        "third should be id_a (Mar): {output}"
    );
}

#[test]
fn test_raw_output_no_chrome() {
    let id = make_id(10_000);
    let multiline = "line-a\nMATCH-here\nline-c";
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from(multiline),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        context: 1,
        raw: true,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert_eq!(lines.len(), 3, "raw context=1: {output}");
    // No conversation ID prefix, no separators
    assert_eq!(lines[0], "line-a");
    assert_eq!(lines[1], "MATCH-here");
    assert_eq!(lines[2], "line-c");
    assert!(
        !output.contains(':'),
        "raw should not contain ':' separator"
    );
}

#[test]
fn test_raw_output_group_separator() {
    let id = make_id(10_100);
    let multiline = "MATCH-first\nline-b\nline-c\nline-d\nMATCH-last";
    let (mut ctx, _, out) = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from(multiline),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        raw: true,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();
    // context=0 with two non-adjacent matches produces two lines with a separator
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "MATCH-first");
    assert_eq!(lines[1], "--");
    assert_eq!(lines[2], "MATCH-last");
}

#[test]
fn test_json_output() {
    let id = make_id(10_200);
    let multiline = "line-a\nMATCH-here\nline-c";
    let (mut ctx, _, out) = setup_ctx_with_json(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from(multiline),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        context: 1,
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    let arr = parsed.as_array().expect("should be array");
    assert_eq!(arr.len(), 3, "context=1 around 1 match: {output}");

    assert_eq!(arr[0]["text"], "line-a");
    assert_eq!(arr[0]["match"], false);

    assert_eq!(arr[1]["text"], "MATCH-here");
    assert_eq!(arr[1]["match"], true);

    assert_eq!(arr[2]["text"], "line-c");
    assert_eq!(arr[2]["match"], false);

    // All entries should have the conversation id
    let id_str = id.to_string();
    for entry in arr {
        assert_eq!(entry["id"], id_str);
    }
}

#[test]
fn test_plain_text_no_ansi() {
    let id = make_id(10_300);
    let (mut ctx, _, out) = setup_ctx_with_plain(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from("MATCH-here"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    let grep = Grep {
        pattern: "MATCH".into(),
        ..Default::default()
    };
    grep.run(&mut ctx, vec![]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    // Plain text mode should not contain ANSI escape sequences
    assert!(
        !output.contains('\x1b'),
        "plain text should have no ANSI: {output}"
    );
    assert!(output.contains(&id.to_string()));
    assert!(output.contains("MATCH-here"));
}

#[test]
fn test_matching_lines_basic() {
    let lines = vec!["foo", "bar", "baz"];
    assert_eq!(matching_lines(&lines, "bar", false), vec![1]);
}

#[test]
fn test_matching_lines_case_insensitive() {
    let lines = vec!["Foo", "BAR", "baz"];
    assert_eq!(matching_lines(&lines, "bar", true), vec![1]);
}

#[test]
fn test_context_ranges_no_overlap() {
    // Indices 0 and 4 with ctx=1 in a 5-line block => (0,1) and (3,4)
    let ranges = context_ranges(&[0, 4], 1, 5);
    assert_eq!(ranges, vec![(0, 1), (3, 4)]);
}

#[test]
fn test_context_ranges_merge() {
    // Indices 1 and 3 with ctx=1 in a 5-line block => merged to (0,4)
    let ranges = context_ranges(&[1, 3], 1, 5);
    assert_eq!(ranges, vec![(0, 4)]);
}

#[test]
fn test_context_ranges_single() {
    let ranges = context_ranges(&[2], 0, 5);
    assert_eq!(ranges, vec![(2, 2)]);
}

#[test]
fn test_event_text_content_chat_request() {
    let kind = EventKind::ChatRequest("hello world".into());
    let texts = event_lines(&kind);
    assert_eq!(texts, vec!["hello world"]);
}

#[test]
fn test_event_text_content_chat_response_message() {
    let kind = EventKind::ChatResponse(ChatResponse::message("response text"));
    let texts = event_lines(&kind);
    assert_eq!(texts, vec!["response text"]);
}

#[test]
fn test_event_text_content_chat_response_reasoning() {
    let kind = EventKind::ChatResponse(ChatResponse::reasoning("thinking..."));
    let texts = event_lines(&kind);
    assert_eq!(texts, vec!["thinking..."]);
}

#[test]
fn test_event_text_content_turn_start() {
    let kind = EventKind::TurnStart(jp_conversation::event::TurnStart);
    let texts = event_lines(&kind);
    assert!(texts.is_empty());
}
