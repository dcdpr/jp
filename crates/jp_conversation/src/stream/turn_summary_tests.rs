//! Tests for `TurnStart::summary`, `Turn` summary accessors, and
//! `ConversationStream` summary mutations.

use chrono::{DateTime, Utc};
use jp_config::PartialAppConfig;
use serde_json::{from_str, json, to_string};

use super::{ConversationStream, EventPayload, InternalEvent, StoredEvent};
use crate::{
    ConversationEvent, EventId,
    event::{ChatRequest, ChatResponse, TurnStart},
};

fn entry(id: &str, event: ConversationEvent) -> InternalEvent {
    InternalEvent {
        event_id: EventId::fixed(id),
        payload: EventPayload::Event(Box::new(event)),
    }
}

fn event(kind: impl Into<ConversationEvent>) -> ConversationEvent {
    let mut event = kind.into();
    event.timestamp = DateTime::<Utc>::UNIX_EPOCH;
    event
}

fn summarized(summary: &str) -> TurnStart {
    TurnStart {
        summary: Some(summary.to_owned()),
    }
}

fn fixture() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("turn001", event(summarized("Set up the project"))),
        entry("chat001", event(ChatRequest::from("first question"))),
        entry("reply01", event(ChatResponse::message("first answer"))),
        entry("turn002", event(TurnStart::default())),
        entry("chat002", event(ChatRequest::from("second question"))),
    ]);
    stream
}

fn summaries(stream: &ConversationStream) -> Vec<Option<String>> {
    stream
        .iter_turns()
        .map(|turn| turn.summary().map(str::to_owned))
        .collect()
}

#[test]
fn a_summary_is_stored_on_the_turn_start_entry() {
    let stored = to_string(&entry(
        "k3m9x2a",
        event(summarized("Add retry backoff to stream handler")),
    ))
    .unwrap();

    assert_eq!(
        stored,
        r#"{"event_id":"k3m9x2a","timestamp":"1970-01-01 00:00:00.0","type":"turn_start","summary":"Add retry backoff to stream handler"}"#
    );
}

#[test]
fn a_turn_start_without_a_summary_omits_the_summary_key() {
    let stored = to_string(&entry("k3m9x2a", event(TurnStart::default()))).unwrap();

    assert_eq!(
        stored,
        r#"{"event_id":"k3m9x2a","timestamp":"1970-01-01 00:00:00.0","type":"turn_start"}"#
    );
}

#[test]
fn a_stored_summary_reads_back_onto_the_turn_start() {
    let raw = r#"{"event_id":"k3m9x2a","timestamp":"1970-01-01 00:00:00.0","type":"turn_start","summary":"Add retry backoff to stream handler"}"#;

    let read = from_str::<StoredEvent>(raw).unwrap().into_entry();

    assert_eq!(
        read,
        entry(
            "k3m9x2a",
            event(summarized("Add retry backoff to stream handler"))
        )
    );
    assert_eq!(to_string(&read).unwrap(), raw);
}

#[test]
fn a_legacy_turn_start_deserializes_with_no_summary() {
    let (base_config, _) = ConversationStream::new_test().to_parts().unwrap();
    let stream = ConversationStream::from_parts(
        base_config,
        vec![
            json!({"type": "turn_start", "timestamp": "1970-01-01 00:00:00.0"}),
            json!({"type": "chat_request", "timestamp": "1970-01-01 00:00:00.0", "content": "first question"}),
            json!({"event_id": "turn002", "type": "turn_start", "timestamp": "1970-01-01 00:00:00.0"}),
            json!({"event_id": "chat002", "type": "chat_request", "timestamp": "1970-01-01 00:00:00.0", "content": "second question"}),
        ],
        &PartialAppConfig::empty(),
    )
    .unwrap();

    assert_eq!(summaries(&stream), [None, None]);
}

#[test]
fn a_summary_survives_a_save_and_reload() {
    let (base_config, events) = fixture().to_parts().unwrap();

    let reloaded =
        ConversationStream::from_parts(base_config, events, &PartialAppConfig::empty()).unwrap();

    assert_eq!(summaries(&reloaded), [
        Some("Set up the project".to_owned()),
        None
    ]);
}

#[test]
fn a_turn_reports_the_id_of_its_turn_start() {
    let stream = fixture();

    let ids: Vec<_> = stream
        .iter_turns()
        .map(|turn| turn.id().map(ToString::to_string))
        .collect();

    assert_eq!(ids, [
        Some("turn001".to_owned()),
        Some("turn002".to_owned())
    ]);
}

#[test]
fn a_turn_reports_the_summary_on_its_turn_start() {
    assert_eq!(summaries(&fixture()), [
        Some("Set up the project".to_owned()),
        None
    ]);
}

#[test]
fn an_implicit_leading_turn_has_no_id_or_summary() {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("chat000", event(ChatRequest::from("orphan"))),
        entry("turn001", event(summarized("Set up the project"))),
        entry("chat001", event(ChatRequest::from("first question"))),
    ]);

    let turns: Vec<_> = stream.iter_turns().collect();

    assert_eq!(turns[0].id(), None);
    assert_eq!(turns[0].summary(), None);
    assert_eq!(turns[1].id(), Some(&EventId::fixed("turn001")));
    assert_eq!(turns[1].summary(), Some("Set up the project"));
}

#[test]
fn fallback_summary_uses_the_first_non_empty_request_line() {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("turn001", event(TurnStart::default())),
        entry(
            "chat001",
            event(ChatRequest::from("\n   \n  Fix   the\tretry test  \nIt fails on CI.")),
        ),
        entry("reply01", event(ChatResponse::message("Looking."))),
        entry("chat002", event(ChatRequest::from("Also update the docs."))),
    ]);

    let turn = stream.iter_turns().next().unwrap();
    assert_eq!(turn.fallback_summary().as_deref(), Some("Fix the retry test"));
}

#[test]
fn fallback_summary_truncates_at_72_characters() {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("turn001", event(TurnStart::default())),
        entry("chat001", event(ChatRequest::from("é".repeat(72)))),
        entry("turn002", event(TurnStart::default())),
        entry("chat002", event(ChatRequest::from("y".repeat(100)))),
    ]);

    let turns: Vec<_> = stream.iter_turns().collect();
    assert_eq!(turns[0].fallback_summary(), Some("é".repeat(72)));
    assert_eq!(turns[1].fallback_summary(), Some(format!("{}…", "y".repeat(71))));
}

#[test]
fn setting_a_summary_stores_it_on_the_named_turn() {
    let mut stream = fixture();

    let updated = stream.set_turn_summary(
        &EventId::fixed("turn002"),
        "Answer the second question",
    );

    assert!(updated);
    assert_eq!(summaries(&stream), [
        Some("Set up the project".to_owned()),
        Some("Answer the second question".to_owned()),
    ]);
}

#[test]
fn setting_a_summary_normalizes_whitespace_and_rejects_blank_input() {
    let mut stream = fixture();

    assert!(!stream.set_turn_summary(&EventId::fixed("turn002"), "   \n\t  "));
    assert_eq!(summaries(&stream), [Some("Set up the project".to_owned()), None]);

    assert!(stream.set_turn_summary(
        &EventId::fixed("turn002"),
        "  Answer\n  the second\tquestion  ",
    ));
    assert_eq!(summaries(&stream), [
        Some("Set up the project".to_owned()),
        Some("Answer the second question".to_owned()),
    ]);
}

#[test]
fn setting_a_summary_replaces_the_existing_summary() {
    let mut stream = fixture();

    let updated = stream.set_turn_summary(&EventId::fixed("turn001"), "Scaffold the crate");

    assert!(updated);
    assert_eq!(summaries(&stream), [
        Some("Scaffold the crate".to_owned()),
        None
    ]);
}

#[test]
fn setting_a_summary_for_an_unknown_or_non_turn_start_id_returns_false() {
    let mut stream = fixture();

    assert!(!stream.set_turn_summary(&EventId::fixed("missing"), "Lost"));
    assert!(!stream.set_turn_summary(&EventId::fixed("chat002"), "Wrong entry"));
    assert_eq!(stream, fixture());
}

#[test]
fn setting_a_summary_for_a_removed_turn_returns_false() {
    let mut stream = fixture();
    stream.retain_turns(|index| index == 1);

    let updated = stream.set_turn_summary(&EventId::fixed("turn001"), "Set up the project");

    assert!(!updated);
    assert_eq!(summaries(&stream), [None]);
}

#[test]
fn clearing_a_summary_removes_it_from_the_named_turn() {
    let mut stream = fixture();

    assert!(stream.clear_turn_summary(&EventId::fixed("turn001")));
    assert_eq!(summaries(&stream), [None, None]);

    assert!(!stream.clear_turn_summary(&EventId::fixed("missing")));
}

#[test]
fn popping_or_trimming_an_event_clears_the_shortened_turns_summary() {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("turn001", event(summarized("Set up the project"))),
        entry("chat001", event(ChatRequest::from("first question"))),
        entry("reply01", event(ChatResponse::message("first answer"))),
        entry("turn002", event(summarized("Switch retries to backoff"))),
        entry("chat002", event(ChatRequest::from("add retries"))),
        entry("reply02", event(ChatResponse::message("retrying"))),
        entry("chat003", event(ChatRequest::from("use exponential backoff"))),
        entry("reply03", event(ChatResponse::message("done"))),
    ]);

    let trimmed = stream.trim_chat_request();
    assert!(trimmed.is_some());
    assert_eq!(summaries(&stream), [
        Some("Set up the project".to_owned()),
        None,
    ]);
}

#[test]
fn a_summary_stays_with_its_turn_when_earlier_turns_are_dropped() {
    let mut stream = fixture();
    assert!(stream.set_turn_summary(
        &EventId::fixed("turn002"),
        "Answer the second question",
    ));

    stream.retain_last_turns(1);

    assert_eq!(summaries(&stream), [Some(
        "Answer the second question".to_owned()
    )]);
}
