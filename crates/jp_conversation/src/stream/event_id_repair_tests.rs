use std::collections::HashSet;

use jp_config::PartialAppConfig;
use serde_json::{Value, json};

use super::{ConversationStream, fresh_event_id};
use crate::{EventId, event::ChatResponse};

fn load(events: Vec<Value>) -> ConversationStream {
    let (base_config, _) = ConversationStream::new_test().to_parts().unwrap();
    ConversationStream::from_parts(base_config, events, &PartialAppConfig::empty()).unwrap()
}

#[test]
fn from_parts_repairs_later_occurrences_without_sanitizing() {
    let stream = load(vec![
        json!({"event_id": "shared", "type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "chat_response", "message": "answer", "timestamp": "1970-01-01 00:00:01.0"}),
    ]);

    assert_eq!(stream.events.len(), 2, "load must not inject a turn marker");
    assert_eq!(stream.events[0].event_id, EventId::fixed("shared"));
    assert_ne!(stream.events[1].event_id, EventId::fixed("shared"));
    assert_eq!(
        stream.duplicated_event_ids,
        HashSet::from([EventId::fixed("shared")])
    );
    assert_eq!(
        stream.first().unwrap().as_chat_request().unwrap().content,
        "question"
    );
    assert_eq!(
        stream.last().unwrap().as_chat_response().unwrap(),
        &ChatResponse::message("answer")
    );
    let (_, saved) = stream.to_parts().unwrap();
    assert_eq!(saved[1]["timestamp"], "1970-01-01 00:00:01.0");
}

#[test]
fn from_parts_repairs_duplicates_across_every_payload_kind() {
    let mut original = vec![
        json!({"event_id": "shared", "type": "config_delta", "op": "reset", "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "compaction", "from_turn": 0, "to_turn": 0, "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "event_overlay", "patches": [], "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "future_event", "nested": {"event_id": "payload-id"}}),
        json!({"event_id": "shared", "type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
    ];
    let stream = load(original.clone());

    let ids: HashSet<_> = stream.events.iter().map(|event| &event.event_id).collect();
    assert_eq!(ids.len(), 5);
    assert_eq!(stream.events[0].event_id, EventId::fixed("shared"));
    assert_eq!(
        stream.duplicated_event_ids,
        HashSet::from([EventId::fixed("shared")])
    );
    assert_eq!(stream.iter().count(), 1);

    let (_, mut saved) = stream.to_parts().unwrap();
    for event in original.iter_mut().chain(&mut saved) {
        event.as_object_mut().unwrap().shift_remove("event_id");
    }
    assert_eq!(saved, original);
}

#[test]
fn repair_reserves_later_ids_and_generated_replacements() {
    let mut stream = load(vec![
        json!({"event_id": "first", "type": "future_event", "value": 1}),
        json!({"event_id": "second", "type": "future_event", "value": 2}),
        json!({"event_id": "third", "type": "future_event", "value": 3}),
        json!({"event_id": "later", "type": "future_event", "value": 4}),
    ]);
    stream.events[1].event_id = EventId::fixed("first");
    stream.events[2].event_id = EventId::fixed("first");
    let mut candidates = ["later", "first", "fresh01", "fresh01", "fresh02"]
        .map(EventId::fixed)
        .into_iter();

    stream.ensure_unique_event_ids(|| candidates.next().expect("fixed ID candidate"));

    let ids: Vec<_> = stream
        .events
        .iter()
        .map(|event| event.event_id.to_string())
        .collect();
    assert_eq!(ids, ["first", "fresh01", "fresh02", "later"]);
    assert_eq!(
        stream.duplicated_event_ids,
        HashSet::from([EventId::fixed("first")])
    );
    assert!(candidates.next().is_none());
}

#[test]
fn ambiguity_survives_repair_rechecks_and_clears_on_reload() {
    let mut stream = load(vec![
        json!({"event_id": "first", "type": "future_event"}),
        json!({"event_id": "first", "type": "future_event"}),
        json!({"event_id": "second", "type": "future_event"}),
        json!({"event_id": "second", "type": "future_event"}),
    ]);
    let (base_config, saved) = stream.to_parts().unwrap();
    stream.ensure_unique_event_ids(|| panic!("already unique"));
    stream.sanitize();

    assert_eq!(
        stream.duplicated_event_ids,
        HashSet::from([EventId::fixed("first"), EventId::fixed("second")])
    );
    assert_eq!(
        stream.clone().duplicated_event_ids,
        stream.duplicated_event_ids
    );
    let reloaded =
        ConversationStream::from_parts(base_config, saved.clone(), &PartialAppConfig::empty())
            .unwrap();
    assert!(reloaded.duplicated_event_ids.is_empty());
    assert_eq!(reloaded.to_parts().unwrap().1, saved);
}

#[test]
fn legacy_load_assigns_ids_to_entries_without_ids() {
    let (base_config, _) = ConversationStream::new_test().to_parts().unwrap();
    let stream = ConversationStream::from_legacy_events(vec![
        json!({"type": "config_delta", "delta": base_config, "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"type": "future_event", "content": "opaque"}),
    ], &PartialAppConfig::empty()).unwrap().unwrap();

    assert_eq!(stream.events.len(), 2);
    assert_ne!(stream.events[0].event_id, stream.events[1].event_id);
    assert!(stream.duplicated_event_ids.is_empty());
    let (_, saved) = stream.to_parts().unwrap();
    assert!(
        saved
            .iter()
            .all(|event| event["event_id"].as_str().is_some_and(|id| !id.is_empty()))
    );
}

#[test]
fn legacy_load_repairs_duplicates_through_from_parts() {
    let (base_config, _) = ConversationStream::new_test().to_parts().unwrap();
    let stream = ConversationStream::from_legacy_events(vec![
        json!({"type": "config_delta", "delta": base_config, "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "future_event"}),
    ], &PartialAppConfig::empty()).unwrap().unwrap();

    assert_eq!(stream.events[0].event_id, EventId::fixed("shared"));
    assert_ne!(stream.events[1].event_id, EventId::fixed("shared"));
    assert_eq!(
        stream.duplicated_event_ids,
        HashSet::from([EventId::fixed("shared")])
    );
}

#[test]
fn sanitize_does_not_run_id_repair() {
    let mut stream = load(vec![
        json!({"event_id": "first", "type": "future_event"}),
        json!({"event_id": "second", "type": "future_event"}),
    ]);
    stream.events[1].event_id = EventId::fixed("first");
    stream.sanitize();

    assert_eq!(stream.events[0].event_id, stream.events[1].event_id);
    assert!(stream.duplicated_event_ids.is_empty());
}

#[test]
fn fresh_event_id_retries_collisions_with_stream_entries() {
    let stream = load(vec![
        json!({"event_id": "taken01", "type": "future_event"}),
        json!({"event_id": "taken02", "type": "future_event"}),
    ]);
    let mut candidates = ["taken01", "taken02", "fresh01"]
        .map(EventId::fixed)
        .into_iter();
    let id = fresh_event_id(
        |id| stream.events.iter().any(|event| event.event_id == *id),
        || candidates.next().expect("fixed ID candidate"),
    );

    assert_eq!(id, EventId::fixed("fresh01"));
    assert!(candidates.next().is_none());
}
