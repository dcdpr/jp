use std::time::Duration;

use camino_tempfile::tempdir;
use chrono::{TimeZone as _, Utc};
use jp_config::AppConfig;
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId, EventKind,
    event::{ChatRequest, ChatResponse, ToolCallResponse},
};
use jp_printer::{OutputFormat, Printer};
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::Globals;

fn setup_ctx_with_events(events: Vec<(ConversationId, Vec<ConversationEvent>)>) -> Ctx {
    let entries = events
        .into_iter()
        .map(|(id, evts)| (id, Conversation::default(), evts))
        .collect();
    setup_ctx_with_conversations(entries)
}

fn setup_ctx_with_conversations(
    entries: Vec<(ConversationId, Conversation, Vec<ConversationEvent>)>,
) -> Ctx {
    let tmp = tempdir().unwrap();
    let config = AppConfig::new_test();
    let workspace = Workspace::in_memory(tmp.path());
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    let mut ctx = Ctx::new(
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    for (id, conversation, evts) in entries {
        ctx.workspace
            .create_conversation_with_id(id, conversation, ctx.config());
        let h = ctx.workspace.acquire_conversation(&id).unwrap();
        let lock = ctx.workspace.test_lock(h);
        lock.as_mut().update_events(|e| e.extend(evts));
    }

    ctx
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(chrono::DateTime::<Utc>::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap()
}

// --- event_lines / event_scope primitives -----------------------------------

fn collect_lines(kind: &EventKind) -> Vec<String> {
    event_lines(kind)
        .into_iter()
        .map(std::borrow::Cow::into_owned)
        .collect()
}

#[test]
fn event_lines_chat_request() {
    let kind = EventKind::ChatRequest("hello world".into());
    assert_eq!(collect_lines(&kind), vec!["hello world".to_string()]);
}

#[test]
fn event_lines_chat_response_message() {
    let kind = EventKind::ChatResponse(ChatResponse::message("response text"));
    assert_eq!(collect_lines(&kind), vec!["response text".to_string()]);
}

#[test]
fn event_lines_chat_response_reasoning() {
    let kind = EventKind::ChatResponse(ChatResponse::reasoning("thinking..."));
    assert_eq!(collect_lines(&kind), vec!["thinking...".to_string()]);
}

#[test]
fn event_lines_turn_start_is_empty() {
    let kind = EventKind::TurnStart(jp_conversation::event::TurnStart);
    assert!(collect_lines(&kind).is_empty());
}

#[test]
fn event_scope_mapping() {
    assert_eq!(
        event_scope(&EventKind::ChatRequest("x".into())),
        Some(ConcreteScope::User)
    );
    assert_eq!(
        event_scope(&EventKind::ChatResponse(ChatResponse::message("x"))),
        Some(ConcreteScope::Assistant)
    );
    assert_eq!(
        event_scope(&EventKind::ChatResponse(ChatResponse::reasoning("x"))),
        Some(ConcreteScope::Reasoning)
    );
    assert_eq!(
        event_scope(&EventKind::TurnStart(jp_conversation::event::TurnStart)),
        None
    );
}

// --- matcher failure --------------------------------------------------------

#[test]
fn a_poisoned_matcher_stops_matching() {
    // Once a failure is recorded, every result will be discarded, so further
    // engine invocations are wasted work — each can burn the full backtrack
    // limit again. The gate is observable: a line the pattern genuinely
    // matches reports `false` after the poison.
    let matcher = Matcher::new(r"(a+)+\1b", true, false).unwrap();
    assert!(matcher.is_match("aab"), "sanity: pattern matches this line");
    assert!(matcher.failure().is_none());

    // Nested quantifiers over a long same-character run blow the backtrack
    // limit and poison the matcher. The leading `b` is what puts the engine on
    // that path at all: with the pattern's required literal absent from the
    // line, the search is decided without ever entering the backtracking VM.
    // Here the `b` is present but unreachable, since no match can end at it.
    assert!(!matcher.is_match(&format!("b{}", "a".repeat(4000))));
    assert!(matcher.failure().is_some());

    assert!(
        !matcher.is_match("aab"),
        "a poisoned matcher must not run the engine again"
    );
    assert!(
        matcher.find_spans("aab").is_empty(),
        "find_spans shares the gate"
    );
}

// --- filter_ids -------------------------------------------------------------
//
// `filter_ids` uses fixed scopes (title + chat) and smart-case matching, and
// returns matching IDs without building hit metadata. These tests pin the
// scope set and the smart-case rule.

/// The matching IDs, for a pattern expected to compile.
fn matching(ctx: &Ctx, ids: &[ConversationId], pattern: &str) -> Vec<ConversationId> {
    filter_ids(ctx, ids, pattern).unwrap()
}

#[test]
fn filter_ids_matches_chat_request() {
    let id_match = make_id(20_100);
    let id_miss = make_id(20_101);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let ctx = setup_ctx_with_events(vec![
        (id_match, vec![ConversationEvent::new(
            ChatRequest::from("the deployment failed today"),
            ts,
        )]),
        (id_miss, vec![ConversationEvent::new(
            ChatRequest::from("unrelated chat"),
            ts,
        )]),
    ]);

    let matched = matching(&ctx, &[id_match, id_miss], "deployment");
    assert_eq!(matched, vec![id_match]);
}

#[test]
fn filter_ids_matches_chat_response() {
    let id = make_id(20_200);
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatResponse::message("the rollout went smoothly"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    assert_eq!(matching(&ctx, &[id], "rollout"), vec![id]);
}

#[test]
fn filter_ids_matches_reasoning() {
    let id = make_id(20_300);
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatResponse::reasoning("step one is to check the schema"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    assert_eq!(matching(&ctx, &[id], "schema"), vec![id]);
}

#[test]
fn filter_ids_matches_title() {
    let id = make_id(20_400);
    let conv = Conversation {
        title: Some("Refactor the storage layer".into()),
        ..Default::default()
    };
    let ctx = setup_ctx_with_conversations(vec![(id, conv, vec![])]);

    assert_eq!(matching(&ctx, &[id], "storage"), vec![id]);
}

#[test]
fn filter_ids_ignores_tool_call_response() {
    // Tool call results sit outside the chat-style scope set. A match in
    // tool output should NOT pull the conversation into the picker.
    let id = make_id(20_500);
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ToolCallResponse {
            id: "tc1".into(),
            result: Ok("secret-keyword found in file".into()),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    assert!(matching(&ctx, &[id], "secret-keyword").is_empty());
}

#[test]
fn filter_ids_smart_case_lowercase_is_insensitive() {
    let id = make_id(20_600);
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from("Tell me about WASM"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    // All-lowercase pattern → case-insensitive match.
    assert_eq!(matching(&ctx, &[id], "wasm"), vec![id]);
}

#[test]
fn filter_ids_smart_case_uppercase_is_sensitive() {
    let id_lower = make_id(20_700);
    let id_upper = make_id(20_701);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let ctx = setup_ctx_with_events(vec![
        (id_lower, vec![ConversationEvent::new(
            ChatRequest::from("tell me about wasm"),
            ts,
        )]),
        (id_upper, vec![ConversationEvent::new(
            ChatRequest::from("tell me about WASM"),
            ts,
        )]),
    ]);

    // Pattern with an uppercase letter → case-sensitive: only the uppercase
    // conversation matches.
    assert_eq!(matching(&ctx, &[id_lower, id_upper], "WASM"), vec![
        id_upper
    ]);
}

#[test]
fn filter_ids_treats_the_pattern_as_a_literal() {
    // `filter_ids` shares `c grep`'s matcher, which compiles a pattern as an
    // *escaped* regex. If that escaping were ever dropped, `.` would silently
    // become a wildcard here and `c use --grep` would match conversations the
    // user never asked for.
    let id_literal = make_id(20_750);
    let id_wildcard = make_id(20_751);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let ctx = setup_ctx_with_events(vec![
        (id_literal, vec![ConversationEvent::new(
            ChatRequest::from("contains a.c exactly"),
            ts,
        )]),
        (id_wildcard, vec![ConversationEvent::new(
            ChatRequest::from("contains abc instead"),
            ts,
        )]),
    ]);

    assert_eq!(matching(&ctx, &[id_literal, id_wildcard], "a.c"), vec![
        id_literal
    ]);
}

#[test]
fn filter_ids_returns_empty_when_no_match() {
    let id = make_id(20_800);
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatRequest::from("hello world"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    assert!(matching(&ctx, &[id], "nonexistent").is_empty());
}

#[test]
fn filter_ids_preserves_input_order() {
    // Build conversations in non-sequential creation order. `filter_ids`
    // should preserve the input slice order regardless of internal
    // parallelism.
    let id_a = make_id(20_900);
    let id_b = make_id(20_902);
    let id_c = make_id(20_901);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let ctx = setup_ctx_with_events(vec![
        (id_a, vec![ConversationEvent::new(
            ChatRequest::from("shared-marker alpha"),
            ts,
        )]),
        (id_b, vec![ConversationEvent::new(
            ChatRequest::from("shared-marker beta"),
            ts,
        )]),
        (id_c, vec![ConversationEvent::new(
            ChatRequest::from("shared-marker gamma"),
            ts,
        )]),
    ]);

    let input = vec![id_a, id_b, id_c];
    assert_eq!(matching(&ctx, &input, "shared-marker"), input);
}
