use std::time::Duration;

use camino_tempfile::tempdir;
use chrono::{TimeZone as _, Utc};
use jp_config::AppConfig;
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId, EventKind, Labels,
    event::{
        ChatRequest, ChatResponse, InquiryQuestion, InquiryRequest, InquirySource, ToolCallRequest,
        ToolCallResponse,
    },
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
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
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
fn event_lines_chat_response_structured_object() {
    // A structured response is parsed into a `Value` before it is persisted, so
    // the searchable text has to be re-serialized. Pretty-printed, matching how
    // tool call arguments are handled.
    let kind = EventKind::ChatResponse(ChatResponse::structured(serde_json::json!({
        "name": "Alice"
    })));

    assert_eq!(collect_lines(&kind), vec![
        "{".to_string(),
        "  \"name\": \"Alice\"".to_string(),
        "}".to_string(),
    ]);
}

#[test]
fn event_lines_chat_response_structured_string_is_verbatim() {
    // A response whose JSON failed to parse is preserved as a raw string. It is
    // searched as-is rather than re-quoted.
    let kind = EventKind::ChatResponse(ChatResponse::structured(serde_json::Value::String(
        "not json {".to_owned(),
    )));

    assert_eq!(collect_lines(&kind), vec!["not json {".to_string()]);
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
// `filter_ids` searches every scope with smart-case matching, and returns
// matching IDs without building hit metadata. These tests pin the scope set and
// the smart-case rule.

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
fn filter_ids_matches_a_label_key() {
    let id_match = make_id(20_410);
    let id_miss = make_id(20_411);
    let labelled = |key: &str| Conversation {
        labels: Labels::from_iter([(key, ["jp_config"])]),
        ..Default::default()
    };
    let ctx = setup_ctx_with_conversations(vec![
        (id_match, labelled("crate"), vec![]),
        (id_miss, labelled("module"), vec![]),
    ]);

    assert_eq!(matching(&ctx, &[id_match, id_miss], "crate"), vec![
        id_match
    ]);
}

#[test]
fn filter_ids_matches_a_label_value() {
    let id_match = make_id(20_420);
    let id_miss = make_id(20_421);
    let labelled = |value: &str| Conversation {
        labels: Labels::from_iter([("crate", [value])]),
        ..Default::default()
    };
    let ctx = setup_ctx_with_conversations(vec![
        (id_match, labelled("jp_config"), vec![]),
        (id_miss, labelled("jp_llm"), vec![]),
    ]);

    assert_eq!(matching(&ctx, &[id_match, id_miss], "jp_config"), vec![
        id_match
    ]);
}

#[test]
fn filter_ids_matches_a_whole_label_pair() {
    // The pair is searched as one line, so the `key=value` text a user types
    // into `--label` also works as a `--grep` pattern.
    let id = make_id(20_430);
    let conv = Conversation {
        labels: Labels::from_iter([("crate", ["jp_config"])]),
        ..Default::default()
    };
    let ctx = setup_ctx_with_conversations(vec![(id, conv, vec![])]);

    assert_eq!(matching(&ctx, &[id], "crate=jp_config"), vec![id]);
}

#[test]
fn filter_ids_matches_tool_call_response() {
    let id = make_id(20_500);
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ToolCallResponse {
            id: "tc1".into(),
            result: Ok("secret-keyword found in file".into()),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    assert_eq!(matching(&ctx, &[id], "secret-keyword"), vec![id]);
}

#[test]
fn filter_ids_matches_tool_call_request_arguments() {
    // Arguments are serialized on demand rather than stored as text, so this
    // pins the one scope whose searchable content doesn't already exist as a
    // string in the event.
    let id = make_id(20_510);
    let mut arguments = serde_json::Map::new();
    arguments.insert(
        "pattern".to_owned(),
        serde_json::Value::String("integer_literal_enum_has_integer_type".to_owned()),
    );
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ToolCallRequest::new("tc1".into(), "fs_grep_files".into(), arguments),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    assert_eq!(
        matching(&ctx, &[id], "integer_literal_enum_has_integer_type"),
        vec![id]
    );
}

#[test]
fn filter_ids_matches_structured_object() {
    let id = make_id(20_530);
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        ChatResponse::structured(serde_json::json!({ "name": "Alice" })),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    assert_eq!(matching(&ctx, &[id], "Alice"), vec![id]);
}

#[test]
fn filter_ids_matches_inquiry_question() {
    let id = make_id(20_520);
    let ctx = setup_ctx_with_events(vec![(id, vec![ConversationEvent::new(
        InquiryRequest::new(
            "iq1",
            InquirySource::Assistant,
            InquiryQuestion::text("Which migration strategy should I use?".to_owned()),
        ),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    )])]);

    assert_eq!(matching(&ctx, &[id], "migration strategy"), vec![id]);
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
