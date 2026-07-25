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
/// Returns the ctx, conversation id, output buffer, and the runtime (kept alive
/// so `Ctx::drop` can persist without panicking).
fn setup_ctx_with_config(
    config: AppConfig,
    events: Vec<ConversationEvent>,
) -> (Ctx, ConversationId, SharedBuffer, SharedBuffer, Runtime) {
    let tmp = tempdir().unwrap();
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let workspace = Workspace::new(tmp.path());
    let runtime = Runtime::new().unwrap();

    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(output.contains("\"name\": \"Alice\""), "got: {output}");
    assert!(output.contains("```json"), "got: {output}");
}

/// Regression: replay must close the structured `json` fence.
/// Before this fix, `TurnRenderer::flush()` only flushed the chat sub-renderer,
/// so a conversation ending on a `ChatResponse::Structured` printed an opening
///
/// ```json with no matching close.
/// ```
#[test]
fn structured_fence_is_closed_at_end_of_replay() {
    let data = json!({"name": "Alice"});
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(
        ChatResponse::structured(data),
        ts(0, 0, 0),
    )]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());
    let opens = output.matches("```json").count();
    // Closing fence is a backtick triple at line start with nothing after.
    let closes = output.lines().filter(|l| l.trim_end() == "```").count();
    assert_eq!(
        opens, closes,
        "opening and closing ``` fences must match, got opens={opens} closes={closes}, output: \
         {output:?}"
    );
}

/// Regression: a message following a structured response in the same
/// conversation must render OUTSIDE the `json` fence — the fence is closed at
/// the role/content boundary, not left open until end-of-stream.
#[test]
fn structured_response_followed_by_message_closes_fence_first() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Extract"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::structured(json!({"name": "Alice"})),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Thanks"), ts(0, 1, 1)),
        ConversationEvent::new(ChatResponse::message("You're welcome.\n\n"), ts(0, 1, 2)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());

    // The trailing message text must not appear before the JSON fence has
    // been closed. Find the index of the JSON close (a `\`\`\`` line
    // standing alone) that follows the opening `\`\`\`json`, and assert
    // the message body shows up only after it.
    let open_idx = output.find("```json").expect("expected an opening fence");
    let close_idx = output[open_idx..]
        .find("\n```")
        .map(|i| open_idx + i)
        .expect("expected a closing fence after the opening one");
    let welcome_idx = output
        .find("You're welcome.")
        .expect("expected the trailing message text");
    assert!(
        welcome_idx > close_idx,
        "trailing message must render after the JSON fence is closed; output: {output:?}"
    );
}

/// Regression: a message after a structured response *within the same turn*
/// must close the `json` fence first.
/// This pins the structured→message branch in `TurnView::render_chat_response`
/// specifically — no per-turn `reconfigure` runs between the two events, so
/// the close has to come from that branch.
#[test]
fn structured_to_message_in_same_turn_closes_fence_first() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Extract"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::structured(json!({"name": "Alice"})),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(ChatResponse::message("You're welcome.\n\n"), ts(0, 0, 3)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());

    let open_idx = output.find("```json").expect("expected an opening fence");
    let close_idx = output[open_idx..]
        .find("\n```")
        .map(|i| open_idx + i)
        .expect("expected a closing fence after the opening one");
    let welcome_idx = output
        .find("You're welcome.")
        .expect("expected the trailing message text");
    assert!(
        welcome_idx > close_idx,
        "trailing message must render after the JSON fence is closed; output: {output:?}"
    );
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
fn turn_header_shows_turn_number_and_relative_time() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("First question"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("First answer.\n\n"), ts(0, 0, 2)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Second question"), ts(0, 1, 1)),
        ConversationEvent::new(ChatResponse::message("Second answer.\n\n"), ts(0, 1, 2)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());
    let lines: Vec<&str> = output.lines().collect();

    // The first (user) header of each turn carries the 1-based turn number and
    // a relative timestamp.
    let user_headers: Vec<&&str> = lines
        .iter()
        .filter(|l| l.contains("\u{2500}\u{2500} user"))
        .collect();
    assert_eq!(
        user_headers.len(),
        2,
        "expected one user header per turn, got: {output:?}"
    );
    assert!(
        user_headers[0].contains("turn 1,") && user_headers[0].contains("ago"),
        "first turn header should show `turn 1` and a relative time, got: {output:?}"
    );
    assert!(
        user_headers[1].contains("turn 2,") && user_headers[1].contains("ago"),
        "second turn header should show `turn 2` and a relative time, got: {output:?}"
    );

    // The assistant header within a turn is not the first shown header, so it
    // carries no turn detail.
    for line in lines.iter().filter(|l| l.contains("\u{2500}\u{2500} jp")) {
        assert!(
            !line.contains("turn "),
            "only the first shown header in a turn carries the detail, got: {line:?}"
        );
    }
}

#[test]
fn turn_header_detail_on_assistant_first_turn() {
    // A turn whose first shown event is an assistant response (no user request)
    // must still carry the detail on the assistant header. This pins the
    // `ensure_assistant_header` consumption path, which
    // `turn_header_shows_turn_number_and_relative_time` (user-first) does not.
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(
        ChatResponse::message("Answer only.\n\n"),
        ts(0, 0, 0),
    )]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());
    let jp_headers: Vec<&str> = output
        .lines()
        .filter(|l| l.contains("\u{2500}\u{2500} jp"))
        .collect();
    assert_eq!(jp_headers.len(), 1, "got: {output:?}");
    assert!(
        jp_headers[0].contains("turn 1,") && jp_headers[0].contains("ago"),
        "assistant header opening the turn should carry the detail, got: {output:?}"
    );
}

#[test]
fn prints_conversation_by_id() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(
        ChatRequest::from("active conversation content"),
        ts(0, 0, 0),
    )]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(Some(1), None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(Some(2), None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(Some(5), None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
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
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(Some(0), None),
        current_config: false,
        style: None,
        compacted: false,
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

#[test]
fn turn_prints_specific_turn() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("First question"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("First answer.\n\n"), ts(0, 0, 2)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Second question"), ts(0, 1, 1)),
        ConversationEvent::new(ChatResponse::message("Second answer.\n\n"), ts(0, 1, 2)),
        ConversationEvent::new(TurnStart, ts(0, 2, 0)),
        ConversationEvent::new(ChatRequest::from("Third question"), ts(0, 2, 1)),
        ConversationEvent::new(ChatResponse::message("Third answer.\n\n"), ts(0, 2, 2)),
    ]);

    // Print only turn 2.
    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, Some(2)),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    assert!(
        !output.contains("First"),
        "turn 1 should be excluded, got: {output}"
    );
    assert!(
        output.contains("Second question"),
        "turn 2 should be present, got: {output}"
    );
    assert!(
        output.contains("Second answer."),
        "turn 2 response should be present, got: {output}"
    );
    assert!(
        !output.contains("Third"),
        "turn 3 should be excluded, got: {output}"
    );
}

#[test]
fn turn_out_of_range_errors() {
    let (mut ctx, id, _out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Only turn"), ts(0, 0, 1)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, Some(5)),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    assert!(result.is_err(), "should error for out-of-range turn");
}

#[test]
fn turn_zero_errors() {
    let (mut ctx, id, _out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Only turn"), ts(0, 0, 1)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, Some(0)),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    assert!(result.is_err(), "should error for turn 0");
}

#[test]
fn style_brief_hides_reasoning_and_tool_details() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;

    let (mut ctx, id, out, err, _rt) = setup_ctx_with_config(config, vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Explain Rust"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::reasoning("Let me think deeply about this...\n\n"),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("src/main.rs"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("fn main() {}".into()),
            },
            ts(0, 0, 4),
        ),
        ConversationEvent::new(
            ChatResponse::message("Rust is a systems language.\n\n"),
            ts(0, 0, 5),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: Some(PrintStyle::Brief),
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let chrome = strip_ansi(&err.lock());

    // Reasoning should be hidden.
    assert!(
        !output.contains("think deeply"),
        "reasoning should be hidden in brief mode, got: {output}"
    );

    // Tool call header should still show, but without arguments.
    assert!(
        chrome.contains("Calling tool read_file"),
        "tool header should still appear, got: {chrome}"
    );
    assert!(
        !chrome.contains("src/main.rs"),
        "tool arguments should be hidden in brief mode, got: {chrome}"
    );

    // Tool results should not appear.
    assert!(
        !chrome.contains("fn main()"),
        "tool results should be hidden in brief mode, got: {chrome}"
    );

    // Assistant message should still be visible.
    assert!(
        output.contains("Rust is a systems language."),
        "message content should still show, got: {output}"
    );
}

#[test]
fn style_chat_hides_reasoning_and_tool_calls() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;

    let (mut ctx, id, out, err, _rt) = setup_ctx_with_config(config, vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Explain Rust"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::reasoning("Let me think deeply about this...\n\n"),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("src/main.rs"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("fn main() {}".into()),
            },
            ts(0, 0, 4),
        ),
        ConversationEvent::new(
            ChatResponse::message("Rust is a systems language.\n\n"),
            ts(0, 0, 5),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: Some(PrintStyle::Chat),
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let chrome = strip_ansi(&err.lock());

    // Reasoning is hidden.
    assert!(
        !output.contains("think deeply"),
        "reasoning should be hidden in chat mode, got: {output}"
    );

    // No tool call header at all.
    assert!(
        !chrome.contains("Calling tool"),
        "tool call header should be hidden in chat mode, got: {chrome}"
    );
    assert!(
        !chrome.contains("read_file"),
        "tool name should not appear in chat mode, got: {chrome}"
    );
    assert!(
        !chrome.contains("fn main()"),
        "tool results should be hidden in chat mode, got: {chrome}"
    );

    // User and assistant messages are still visible.
    assert!(
        output.contains("Explain Rust"),
        "user message should show, got: {output}"
    );
    assert!(
        output.contains("Rust is a systems language."),
        "assistant message should show, got: {output}"
    );
}

#[test]
fn style_user_shows_only_user_messages() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;

    let (mut ctx, id, out, err, _rt) = setup_ctx_with_config(config, vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Explain Rust"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::reasoning("Let me think deeply about this...\n\n"),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("src/main.rs"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("fn main() {}".into()),
            },
            ts(0, 0, 4),
        ),
        ConversationEvent::new(
            ChatResponse::message("Rust is a systems language.\n\n"),
            ts(0, 0, 5),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: Some(PrintStyle::User),
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let chrome = strip_ansi(&err.lock());

    // The user message is the only visible content.
    assert!(
        output.contains("Explain Rust"),
        "user message should show, got: {output}"
    );
    assert!(
        !output.contains("Rust is a systems language."),
        "assistant message should be hidden in user mode, got: {output}"
    );
    assert!(
        !output.contains("think deeply"),
        "reasoning should be hidden in user mode, got: {output}"
    );
    assert!(
        !chrome.contains("Calling tool"),
        "tool calls should be hidden in user mode, got: {chrome}"
    );
    assert!(
        !chrome.contains("fn main()"),
        "tool results should be hidden in user mode, got: {chrome}"
    );
}

#[test]
fn role_header_renders_user_label_from_author() {
    let mut req = ChatRequest::from("hello");
    req.author = Some("alice".into());

    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(req, ts(0, 0, 0))]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());
    assert!(
        output.contains("── alice "),
        "user header should use the author label, got: {output:?}"
    );
}

#[test]
fn role_header_falls_back_to_user_label_without_author() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![ConversationEvent::new(
        ChatRequest::from("hello"),
        ts(0, 0, 0),
    )]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());
    assert!(
        output.contains("── user "),
        "user header should fall back to `user`, got: {output:?}"
    );
}

#[test]
fn role_header_renders_assistant_label_with_model_suffix() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(ChatRequest::from("hello"), ts(0, 0, 0)),
        ConversationEvent::new(ChatResponse::message("hi"), ts(0, 0, 1)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());
    assert!(
        output.contains("── jp (anthropic/test) "),
        "assistant header should include model suffix, got: {output:?}"
    );
}

#[test]
fn role_header_assistant_appears_once_per_turn() {
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(ChatRequest::from("hello"), ts(0, 0, 0)),
        ConversationEvent::new(ChatResponse::message("first chunk"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("second chunk"), ts(0, 0, 2)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());
    let occurrences = output.matches("── jp ").count();
    assert_eq!(
        occurrences, 1,
        "assistant header should be emitted exactly once per turn, got {occurrences}: {output:?}"
    );
}

#[test]
fn role_header_assistant_emitted_before_first_tool_call() {
    // The assistant's first event of the turn is a tool call (no message
    // first). The header should still appear before it.
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(ChatRequest::from("do it"), ts(0, 0, 0)),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
            },
            ts(0, 0, 1),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("contents".into()),
            },
            ts(0, 0, 2),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    // The header writes to stdout (chat renderer), the tool UI to stderr.
    // Verify the assistant header reached stdout even though the first
    // assistant event of the turn was a tool call.
    let output = strip_ansi(&out.lock());
    assert!(
        output.contains("── jp "),
        "assistant header should appear on stdout before tool call, got: {output:?}"
    );
}

#[test]
fn role_header_does_not_emit_plain_hr_separator() {
    // Regression: the old renderer emitted a `---` HR after the user
    // message. The labeled-header design replaces that. Make sure no plain
    // `---` line shows up between user and assistant content.
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(ChatRequest::from("q"), ts(0, 0, 0)),
        ConversationEvent::new(ChatResponse::message("a"), ts(0, 0, 1)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());
    // A bare `---` line (or its line-rule equivalent) should not appear.
    // The unicode line `───...` IS expected as part of the role headers,
    // but those start with `── <label>` so a line starting with three
    // dashes followed by whitespace/eol is what we're guarding against.
    for line in output.lines() {
        let trimmed = line.trim();
        assert_ne!(
            trimmed, "---",
            "plain `---` separator should not appear, got line: {line:?}"
        );
    }
}

#[test]
fn style_chat_separates_messages_across_hidden_reasoning() {
    // Simulates the common case where the assistant emits
    //   Message("...first.") -> Reasoning("...") -> Message("Now let me...")
    // In chat mode, the reasoning block is hidden. Even so, the two
    // message chunks must not be glued together into the same markdown
    // paragraph — they should be separated by a blank line so the
    // transcript reads naturally.
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Explain"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::message("Let me research the codebase first."),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ChatResponse::reasoning("Thinking privately about next steps."),
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ChatResponse::message("Now let me look at the relevant files."),
            ts(0, 0, 4),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: Some(PrintStyle::Chat),
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());

    assert!(
        !output.contains("first.Now let me"),
        "message chunks should not be glued together across hidden reasoning, got: {output:?}"
    );
    assert!(
        output.contains("first.\n\nNow let me"),
        "message chunks should be separated by a blank line, got: {output:?}"
    );
}

#[test]
fn style_chat_separates_messages_across_hidden_tool_call() {
    // Same concern as above, but with a tool call as the hidden boundary.
    let (mut ctx, id, out, _err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Check it"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::message("Checking the file first."),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("contents".into()),
            },
            ts(0, 0, 4),
        ),
        ConversationEvent::new(ChatResponse::message("Here is what I found."), ts(0, 0, 5)),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: Some(PrintStyle::Chat),
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let output = strip_ansi(&out.lock());

    assert!(
        !output.contains("first.Here is"),
        "message chunks should not be glued together across hidden tool calls, got: {output:?}"
    );
    assert!(
        output.contains("first.\n\nHere is what I found."),
        "message chunks should be separated by a single blank line, got: {output:?}"
    );
    assert!(
        !output.contains("first.\n\n\nHere is"),
        "should not have more than one blank line between chunks, got: {output:?}"
    );
}

#[test]
fn style_full_shows_reasoning_and_untruncated_results() {
    let mut config = AppConfig::new_test();
    // Start with reasoning hidden and results truncated to 1 line.
    config.style.reasoning.display = ReasoningDisplayConfig::Hidden;

    let (mut ctx, id, out, err, _rt) = setup_ctx_with_config(config, vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Check the file"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::reasoning("Let me reason about this carefully.\n\n"),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("src/lib.rs"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("line 1\nline 2\nline 3\nline 4\nline 5".into()),
            },
            ts(0, 0, 4),
        ),
        ConversationEvent::new(
            ChatResponse::message("Here are the contents.\n\n"),
            ts(0, 0, 5),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: Some(PrintStyle::Full),
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let result = print.run(&mut ctx, &[h]);
    ctx.printer.flush();

    result.unwrap();
    let output = out.lock().clone();
    let chrome = strip_ansi(&err.lock());

    // Reasoning should be visible despite config hiding it.
    assert!(
        output.contains("reason about this carefully"),
        "reasoning should be shown in full mode, got: {output}"
    );

    // Tool arguments should be visible.
    assert!(
        chrome.contains("src/lib.rs"),
        "tool arguments should be shown in full mode, got: {chrome}"
    );

    // All result lines should be visible (not truncated).
    assert!(
        chrome.contains("line 5"),
        "all result lines should be shown in full mode, got: {chrome}"
    );

    // Message still visible.
    assert!(
        output.contains("Here are the contents."),
        "message should still show, got: {output}"
    );
}

/// A tool call replayed after a reasoning block continues the reasoning region,
/// so its chrome (stderr) carries the reasoning background.
/// `AppConfig::new_test` defaults `style.reasoning.background` to ANSI 236 and
/// `display` to `full`.
#[test]
fn replay_shades_tool_chrome_after_reasoning() {
    let (mut ctx, id, _out, err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("read it"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::reasoning("Let me check the file.\n\n"),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("contents".into()),
            },
            ts(0, 0, 4),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let chrome = err.lock().clone();
    assert!(
        chrome.contains("\x1b[48;5;236m"),
        "replayed tool chrome should carry the reasoning background, got: {chrome:?}"
    );
}

/// With `style.reasoning.extend_across_tool_calls` disabled, replay restores
/// the per-block behaviour: the tool chrome is not shaded even when it follows
/// reasoning.
#[test]
fn replay_does_not_shade_tool_chrome_when_extension_disabled() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.extend_across_tool_calls = false;
    let (mut ctx, id, _out, err, _rt) = setup_ctx_with_config(config, vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("read it"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::reasoning("Let me check the file.\n\n"),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("contents".into()),
            },
            ts(0, 0, 4),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let chrome = err.lock().clone();
    assert!(
        !chrome.contains("\x1b[48;5;236m"),
        "tool chrome must stay unshaded with the extension disabled, got: {chrome:?}"
    );
}

/// With `style.tool_call.show = false`, replay suppresses tool chrome entirely,
/// matching the live path.
/// Before the predicate was wired into `TurnRenderer`, `jp conversation print`
/// ignored `show` and always rendered the chrome.
#[test]
fn replay_suppresses_tool_chrome_when_show_disabled() {
    let mut config = AppConfig::new_test();
    config.style.tool_call.show = false;
    let (mut ctx, id, _out, err, _rt) = setup_ctx_with_config(config, vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("read it"), ts(0, 0, 1)),
        ConversationEvent::new(
            ChatResponse::reasoning("Let me check the file.\n\n"),
            ts(0, 0, 2),
        ),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
            },
            ts(0, 0, 3),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("contents".into()),
            },
            ts(0, 0, 4),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(None, None),
        current_config: false,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let chrome = err.lock().clone();
    assert!(
        !chrome.contains("Calling tool"),
        "tool chrome must be suppressed when style.tool_call.show is false, got: {chrome:?}"
    );
}

/// A reasoning region must not leak across turns.
/// `--current-config` (`ConfigSource::Fixed`) skips the per-turn renderer
/// rebuild, so a single `ChatRenderer` persists across turns: a turn that ends
/// with reasoning followed by a turn that opens with a tool call must still
/// render that tool's chrome unshaded.
#[test]
fn replay_does_not_leak_reasoning_region_across_turns() {
    let (mut ctx, id, _out, err, _rt) = setup_ctx(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("think about it"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::reasoning("Deep thought.\n\n"), ts(0, 0, 2)),
        ConversationEvent::new(TurnStart, ts(0, 0, 3)),
        ConversationEvent::new(ChatRequest::from("now act"), ts(0, 0, 4)),
        ConversationEvent::new(
            ToolCallRequest {
                id: "tc1".into(),
                name: "read_file".into(),
                arguments: Map::from_iter([("path".into(), json!("a.rs"))]),
            },
            ts(0, 0, 5),
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "tc1".into(),
                result: Ok("contents".into()),
            },
            ts(0, 0, 6),
        ),
    ]);

    let print = Print {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        range: TurnRange::from_last_turn(Some(2), None),
        current_config: true,
        style: None,
        compacted: false,
    };
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    print.run(&mut ctx, &[h]).unwrap();
    ctx.printer.flush();

    let chrome = err.lock().clone();
    assert!(
        chrome.contains("Calling tool"),
        "the tool chrome itself should render, got: {chrome:?}"
    );
    assert!(
        !chrome.contains("\x1b[48;5;236m"),
        "a tool call opening a new turn must not carry the previous turn's reasoning background, \
         got: {chrome:?}"
    );
}
