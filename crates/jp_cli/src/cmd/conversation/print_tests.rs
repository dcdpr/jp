use std::time::Duration;

use camino_tempfile::tempdir;
use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::{
    AppConfig,
    style::reasoning::{ReasoningDisplayConfig, TruncateChars},
};
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse, TurnStart},
};
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_workspace::Workspace;
use serde_json::{Map, json};
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals,
    cmd::{conversation_id::PositionalIds, target::ConversationTarget},
    ctx::Ctx,
};

/// Strip ANSI escape codes for readable assertions.
fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8(bytes).expect("valid utf-8 after stripping ANSI")
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + Duration::from_secs(secs)).unwrap()
}

fn ts(h: u32, m: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2020, 1, 1, h, m, s).unwrap()
}

/// Create a `Ctx` backed by an in-memory printer.
///
/// Returns the ctx, conversation id, output buffer, and the runtime (kept
/// alive so `Ctx::drop` can persist without panicking).
fn setup_ctx_with_config(
    config: AppConfig,
    events: Vec<ConversationEvent>,
) -> (Ctx, ConversationId, SharedBuffer, SharedBuffer, Runtime) {
    let tmp = tempdir().unwrap();
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let workspace = Workspace::new(tmp.path());
    let runtime = Runtime::new().unwrap();

    let mut ctx = Ctx::new(
        workspace,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    let id = make_id(1000);
    ctx.workspace
        .create_conversation_with_id(id, Conversation::default(), ctx.config());
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let lock = ctx.workspace.test_lock(h);
    lock.as_mut().update_events(|e| e.extend(events));

    (ctx, id, out, err, runtime)
}

fn setup_ctx(
    events: Vec<ConversationEvent>,
) -> (Ctx, ConversationId, SharedBuffer, SharedBuffer, Runtime) {
    setup_ctx_with_config(AppConfig::new_test(), events)
}

#[test]
fn prints_user_message() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(
        ChatRequest::from("Hello world"),
        ts(0, 0, 0),
    )]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(output.contains("Hello world"), "got: {output}");
}

#[test]
fn prints_assistant_message() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(
        ChatResponse::message("The answer is 42.\n\n"),
        ts(0, 0, 1),
    )]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(output.contains("The answer is 42."), "got: {output}");
}

#[test]
fn prints_reasoning_full() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;

    let (mut ctx, id, out, _err, _rt) = setup_ctx_with_config(config, vec![
        ConversationEvent::new(
            ChatResponse::reasoning("Let me think about this...\n\n"),
            ts(0, 0, 0),
        ),
        ConversationEvent::new(ChatResponse::message("Here is my answer.\n\n"), ts(0, 0, 1)),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(
        output.contains("Let me think about this..."),
        "reasoning should be visible in Full mode, got: {output}"
    );
    assert!(output.contains("Here is my answer."), "got: {output}");
}

#[test]
fn hides_reasoning_when_hidden() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Hidden;

    let (mut ctx, id, out, _err, _rt) = setup_ctx_with_config(config, vec![
        ConversationEvent::new(ChatResponse::reasoning("Secret thoughts\n\n"), ts(0, 0, 0)),
        ConversationEvent::new(ChatResponse::message("Visible answer.\n\n"), ts(0, 0, 1)),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(
        !output.contains("Secret thoughts"),
        "reasoning should be hidden, got: {output}"
    );
    assert!(output.contains("Visible answer."), "got: {output}");
}

#[test]
fn truncates_reasoning() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display =
        ReasoningDisplayConfig::Truncate(TruncateChars { characters: 10 });

    let (mut ctx, id, out, _err, _rt) =
        setup_ctx_with_config(config, vec![ConversationEvent::new(
            ChatResponse::reasoning("This is a very long reasoning chain that goes on and on"),
            ts(0, 0, 0),
        )]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(output.contains("This is a "), "got: {output}");
    assert!(output.contains("..."), "should be truncated, got: {output}");
    assert!(
        !output.contains("goes on and on"),
        "long tail should be cut, got: {output}"
    );
}

#[test]
fn prints_tool_call_and_result() {
    let (mut ctx, id, _out, err, _rt) = setup_ctx(vec![
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("src/main.rs"))]),
            },
            ts(0, 0, 0),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("fn main() {}".into()),
            },
            ts(0, 0, 1),
        ),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let chrome = err.lock().clone();
    let plain = strip_ansi(&chrome);
    assert!(
        plain.contains("Calling tool read_file"),
        "should show tool call header in stderr, got: {plain}"
    );
}

#[test]
fn prints_structured_data() {
    let data = json!({"name": "Alice", "age": 30});
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(
        ChatResponse::structured(data.clone()),
        ts(0, 0, 0),
    )]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(output.contains("\"name\": \"Alice\""), "got: {output}");
    assert!(output.contains("```json"), "got: {output}");
}

#[test]
fn turn_separators_between_turns() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("First question"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("First answer.\n\n"), ts(0, 0, 2)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Second question"), ts(0, 1, 1)),
        ConversationEvent::new(ChatResponse::message("Second answer.\n\n"), ts(0, 1, 2)),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(output.contains("First question"), "got: {output}");
    assert!(output.contains("Second question"), "got: {output}");
}

#[test]
fn prints_conversation_by_id() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(
        ChatRequest::from("active conversation content"),
        ts(0, 0, 0),
    )]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(
        output.contains("active conversation content"),
        "got: {output}"
    );
}

#[test]
fn empty_conversation_produces_no_content() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let trimmed = output.trim();
    assert!(
        trimmed.is_empty(),
        "empty conversation should produce no content, got: {trimmed:?}"
    );
}

#[test]
fn full_conversation_round_trip() {
    let (mut ctx, id, out, err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("What is Rust?"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::message("Rust is a systems programming language focused on safety.\n\n"),
            ts(0, 0, 3),
        ),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Show me an example"), ts(0, 1, 1)),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "write_file".into(),
                arguments: Map::from_iter([("path".into(), json!("example.rs"))]),
            },
            ts(0, 1, 2),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("fn main() { println!(\"Hello\"); }".into()),
            },
            ts(0, 1, 3),
        ),
        ConversationEvent::new(
            ChatResponse::message("Here's a simple Rust program.\n\n"),
            ts(0, 1, 4),
        ),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let plain = strip_ansi(&output);

    assert!(plain.contains("What is Rust?"), "got: {plain}");
    assert!(
        plain.contains("systems programming language"),
        "got: {plain}"
    );
    assert!(plain.contains("Show me an example"), "got: {plain}");
    let chrome = strip_ansi(&err.lock());
    assert!(
        chrome.contains("Calling tool write_file"),
        "tool header should be in stderr, got: {chrome}"
    );
    assert!(plain.contains("simple Rust program"), "got: {plain}");
}

#[test]
fn last_prints_only_last_turn() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("First question"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("First answer.\n\n"), ts(0, 0, 2)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Second question"), ts(0, 1, 1)),
        ConversationEvent::new(ChatResponse::message("Second answer.\n\n"), ts(0, 1, 2)),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: Some(1),
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(
        !output.contains("First question"),
        "first turn should be excluded, got: {output}"
    );
    assert!(
        output.contains("Second question"),
        "last turn should be present, got: {output}"
    );
    assert!(
        output.contains("Second answer."),
        "last turn response should be present, got: {output}"
    );
}

#[test]
fn last_two_with_three_turns() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Turn one"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("Answer one.\n\n"), ts(0, 0, 2)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Turn two"), ts(0, 1, 1)),
        ConversationEvent::new(ChatResponse::message("Answer two.\n\n"), ts(0, 1, 2)),
        ConversationEvent::new(TurnStart, ts(0, 2, 0)),
        ConversationEvent::new(ChatRequest::from("Turn three"), ts(0, 2, 1)),
        ConversationEvent::new(ChatResponse::message("Answer three.\n\n"), ts(0, 2, 2)),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: Some(2),
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(
        !output.contains("Turn one"),
        "first turn should be excluded, got: {output}"
    );
    assert!(output.contains("Turn two"), "got: {output}");
    assert!(output.contains("Turn three"), "got: {output}");
}

#[test]
fn last_exceeding_turn_count_prints_all() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Only question"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("Only answer.\n\n"), ts(0, 0, 2)),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: Some(5),
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(
        output.contains("Only question"),
        "should print everything when --last exceeds turn count, got: {output}"
    );
}

#[test]
fn blank_line_between_tool_calls_and_message() {
    let (mut ctx, id, out, err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Check this"), ts(0, 0, 1)),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
            },
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("file contents".into()),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ChatResponse::message("Here is what I found.\n\n"),
            ts(0, 0, 4),
        ),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let chrome = err.lock().clone();

    // The tool call output (stderr) should be followed by a blank line
    // before the assistant message (stdout) resumes.
    // ChatRenderer's flush_on_transition inserts this blank line
    // on the stdout side when transitioning from ToolCall → Message.
    assert!(
        output.contains("\n\nHere is what I found."),
        "should have blank line before message after tool calls, got stdout: {output:?}, stderr: \
         {chrome:?}"
    );
}

#[test]
fn blank_line_between_message_and_tool_calls() {
    let (mut ctx, id, out, err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Help me"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::message("Let me check something.\n\n"),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "grep_files".into(),
                arguments: Map::from_iter([("pattern".into(), json!("foo"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("match found".into()),
            },
            ts(0, 0, 4),
        ),
        ConversationEvent::new(ChatResponse::message("Found it.\n\n"), ts(0, 0, 5)),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let chrome = strip_ansi(&err.lock());

    // The message should end with its natural trailing newlines, and the
    // tool call line should appear on stderr after that.
    assert!(
        output.contains("Let me check something."),
        "message before tools should be present, got: {output:?}"
    );
    assert!(
        chrome.contains("Calling tool grep_files"),
        "tool call should appear in stderr, got: {chrome:?}"
    );
    // After tool calls, the next message should be separated.
    assert!(
        output.contains("\n\nFound it."),
        "should have blank line before message after tool calls, got: {output:?}"
    );
}

#[test]
fn no_extra_blank_line_between_consecutive_tool_calls() {
    let (mut ctx, id, _out, err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Do two things"), ts(0, 0, 1)),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
            },
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("contents a".into()),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc2".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("b.rs"))]),
            },
            ts(0, 0, 4),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc2".into(),
                result: Ok("contents b".into()),
            },
            ts(0, 0, 5),
        ),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: None,
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let chrome = strip_ansi(&err.lock());

    // Both tool calls should appear in stderr without extra blank lines
    // between them. The ToolRenderer writes each with writeln!, so they
    // should be on consecutive lines.
    let lines: Vec<&str> = chrome.lines().collect();
    let tool_lines: Vec<&&str> = lines
        .iter()
        .filter(|l| l.starts_with("Calling tool"))
        .collect();
    assert_eq!(
        tool_lines.len(),
        2,
        "should have exactly two tool call lines, got: {chrome:?}"
    );
}

#[test]
fn last_zero_prints_nothing() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Hello"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("World.\n\n"), ts(0, 0, 2)),
    ]);

    let print = Print {
        target: PositionalIds {
            ids: vec![ConversationTarget::Id(id)],
        },
        last: Some(0),
        current_config: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let trimmed = output.trim();
    assert!(
        trimmed.is_empty(),
        "--last 0 should produce no content, got: {trimmed:?}"
    );
}
