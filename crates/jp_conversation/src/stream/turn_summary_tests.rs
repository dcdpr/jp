//! Turn summaries: stored on a turn's `TurnStart` entry, read through `Turn`,
//! and written by the entry ID of that `TurnStart`.

use chrono::{DateTime, Utc};
use jp_config::PartialAppConfig;
use serde_json::{from_str, json, to_string};

use super::{ConversationStream, EventPayload, InternalEvent, StoredEvent};
use crate::{
    ConversationEvent, EventId,
    event::{ChatRequest, ChatResponse, TurnStart},
};

/// A stream entry with a known ID, for assertions that name one.
fn entry(id: &str, event: ConversationEvent) -> InternalEvent {
    InternalEvent {
        event_id: EventId::fixed(id),
        payload: EventPayload::Event(Box::new(event)),
    }
}

/// An event at the epoch, so a timestamp never varies between runs.
fn event(kind: impl Into<ConversationEvent>) -> ConversationEvent {
    let mut event = kind.into();
    event.timestamp = DateTime::<Utc>::UNIX_EPOCH;
    event
}

/// A turn marker carrying `summary`.
fn summarized(summary: &str) -> TurnStart {
    TurnStart {
        summary: Some(summary.to_owned()),
    }
}

/// Two turns with known IDs, of which only the first has a summary.
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

/// Each turn's summary, in turn order.
fn summaries(stream: &ConversationStream) -> Vec<Option<String>> {
    stream
        .iter_turns()
        .map(|turn| turn.summary().map(str::to_owned))
        .collect()
}

// ---------------------------------------------------------------------------
// Storage shape
// ---------------------------------------------------------------------------

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
fn a_turn_start_without_a_summary_is_stored_without_the_key() {
    // The bytes a turn marker was stored as before it could carry a summary, so
    // a conversation that never gets one is unchanged on disk.
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
fn a_legacy_turn_start_reads_as_having_no_summary() {
    // Two generations of files written before turns carried summaries: the
    // first turn marker predates entry IDs as well, the second does not.
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

// ---------------------------------------------------------------------------
// Reading through `Turn`
// ---------------------------------------------------------------------------

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
    // Events before the first `TurnStart` form a turn of their own, with no
    // marker to take an ID or a summary from.
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

// ---------------------------------------------------------------------------
// Writing by ID
// ---------------------------------------------------------------------------

#[test]
fn setting_a_summary_stores_it_on_the_named_turn() {
    let mut stream = fixture();

    let updated = stream.set_turn_summary(
        &EventId::fixed("turn002"),
        "Answer the second question".to_owned(),
    );

    assert!(updated);
    assert_eq!(summaries(&stream), [
        Some("Set up the project".to_owned()),
        Some("Answer the second question".to_owned()),
    ]);
}

#[test]
fn setting_a_summary_replaces_the_one_already_stored() {
    let mut stream = fixture();

    let updated =
        stream.set_turn_summary(&EventId::fixed("turn001"), "Scaffold the crate".to_owned());

    assert!(updated);
    assert_eq!(summaries(&stream), [
        Some("Scaffold the crate".to_owned()),
        None
    ]);
}

#[test]
fn setting_a_summary_for_an_unknown_id_changes_nothing() {
    let mut stream = fixture();

    let updated = stream.set_turn_summary(&EventId::fixed("missing"), "Lost".to_owned());

    assert!(!updated);
    assert_eq!(stream, fixture());
}

#[test]
fn setting_a_summary_on_an_entry_that_is_not_a_turn_start_changes_nothing() {
    // The request's ID names an entry inside the turn, not the marker that
    // holds the turn's summary.
    let mut stream = fixture();

    let updated = stream.set_turn_summary(&EventId::fixed("chat002"), "Wrong entry".to_owned());

    assert!(!updated);
    assert_eq!(stream, fixture());
}

#[test]
fn setting_a_summary_for_a_removed_turn_changes_nothing() {
    // A summary generated for a turn that was dropped in the meantime has
    // nowhere to go.
    let mut stream = fixture();
    stream.retain_turns(|index| index == 1);

    let updated =
        stream.set_turn_summary(&EventId::fixed("turn001"), "Set up the project".to_owned());

    assert!(!updated);
    assert_eq!(summaries(&stream), [None]);
}

#[test]
fn setting_a_summary_for_an_id_the_load_found_twice_changes_nothing() {
    // Two turn markers shared one ID in the file. Repair left it on the first,
    // but a summary generated for that ID cannot say which turn it describes.
    let (base_config, _) = ConversationStream::new_test().to_parts().unwrap();
    let mut stream = ConversationStream::from_parts(
        base_config,
        vec![
            json!({"event_id": "turn001", "type": "turn_start", "timestamp": "1970-01-01 00:00:00.0"}),
            json!({"event_id": "chat001", "type": "chat_request", "timestamp": "1970-01-01 00:00:00.0", "content": "first question"}),
            json!({"event_id": "turn001", "type": "turn_start", "timestamp": "1970-01-01 00:00:00.0"}),
            json!({"event_id": "chat002", "type": "chat_request", "timestamp": "1970-01-01 00:00:00.0", "content": "second question"}),
        ],
        &PartialAppConfig::empty(),
    )
    .unwrap();

    let updated = stream.set_turn_summary(&EventId::fixed("turn001"), "Ambiguous".to_owned());

    assert!(!updated);
    assert_eq!(summaries(&stream), [None, None]);
}

#[test]
fn clearing_a_summary_removes_it_from_the_named_turn() {
    let mut stream = fixture();

    let updated = stream.clear_turn_summary(&EventId::fixed("turn001"));

    assert!(updated);
    assert_eq!(summaries(&stream), [None, None]);
}

#[test]
fn clearing_a_summary_for_an_unknown_id_changes_nothing() {
    let mut stream = fixture();

    let updated = stream.clear_turn_summary(&EventId::fixed("missing"));

    assert!(!updated);
    assert_eq!(stream, fixture());
}

#[test]
fn a_summary_stays_with_its_turn_when_earlier_turns_are_dropped() {
    let mut stream = fixture();
    assert!(stream.set_turn_summary(
        &EventId::fixed("turn002"),
        "Answer the second question".to_owned(),
    ));

    stream.retain_last_turns(1);

    assert_eq!(summaries(&stream), [Some(
        "Answer the second question".to_owned()
    )]);
}
