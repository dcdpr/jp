//! Load-time repair of entry IDs: missing, empty, and duplicate values.

use std::collections::HashSet;

use jp_config::PartialAppConfig;
use serde_json::{Value, json};

use super::ConversationStream;
use crate::EventId;

/// An entry a build does not recognize, kept verbatim.
///
/// Used for most fixtures here because repair is indifferent to payload kind,
/// and an unknown entry needs no valid body to be loadable.
fn opaque(id: &str) -> Value {
    json!({"event_id": id, "type": "future_event"})
}

/// Load raw entries the way the storage layer does, running repair.
fn load(events: Vec<Value>) -> ConversationStream {
    let (base_config, _) = ConversationStream::new_test().to_parts().unwrap();
    ConversationStream::from_parts(base_config, events, &PartialAppConfig::empty()).unwrap()
}

/// The IDs a loaded stream holds, in stream order.
fn ids(stream: &ConversationStream) -> Vec<String> {
    stream
        .events
        .iter()
        .map(|event| event.event_id.to_string())
        .collect()
}

#[test]
fn the_first_occurrence_of_a_duplicate_keeps_its_id() {
    let stream = load(vec![opaque("shared"), opaque("shared")]);

    assert_eq!(ids(&stream)[0], "shared");
}

#[test]
fn a_later_occurrence_of_a_duplicate_is_reassigned() {
    let stream = load(vec![opaque("shared"), opaque("shared")]);

    assert_ne!(ids(&stream)[1], "shared");
}

#[test]
fn a_duplicated_id_is_recorded_as_ambiguous() {
    // The surviving entry keeps the ID, so the ambiguity is not "which entry
    // exists" but "which entry a reference meant", which is why the value is
    // reported rather than retired.
    let stream = load(vec![opaque("shared"), opaque("shared")]);

    assert_eq!(
        stream.duplicated_event_ids(),
        &HashSet::from([EventId::fixed("shared")])
    );
}

#[test]
fn a_replacement_avoids_an_id_the_repair_pass_has_not_reached() {
    // `shared` is duplicated before `taken` appears, so a pass that reserved
    // IDs as it walked, rather than reading the whole file up front, could
    // hand the replacement an ID a later entry already holds.
    //
    // Generated IDs make a real collision here vanishingly unlikely, so what
    // this pins is that the reservation covers the later entry at all:
    // `event_ids` is seeded from every loaded entry before repair runs, which
    // `EventIds::fresh` then draws against. See
    // `a_draw_retries_past_an_id_the_set_already_holds` for the deterministic
    // half.
    let stream = load(vec![opaque("shared"), opaque("shared"), opaque("taken")]);
    let ids = ids(&stream);

    assert_eq!(ids[0], "shared");
    assert_eq!(ids[2], "taken");
    assert_ne!(ids[1], "shared");
    assert_ne!(ids[1], "taken");
}

#[test]
fn every_entry_holds_a_distinct_id_after_repair() {
    let stream = load(vec![
        opaque("shared"),
        opaque("shared"),
        opaque("shared"),
        opaque("shared"),
    ]);

    let unique: std::collections::HashSet<_> = ids(&stream).into_iter().collect();
    assert_eq!(unique.len(), 4);
}

#[test]
fn repair_reaches_every_payload_kind() {
    // Repair works on the wrapper, so each payload kind must be reachable by
    // it. Each entry below carries the same ID, so all but the first are
    // reassigned.
    let stream = load(vec![
        json!({"event_id": "shared", "type": "config_delta", "op": "reset", "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "compaction", "from_turn": 0, "to_turn": 0, "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "event_overlay", "patches": [], "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "future_event"}),
        json!({"event_id": "shared", "type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
    ]);

    let unique: std::collections::HashSet<_> = ids(&stream).into_iter().collect();
    assert_eq!(unique.len(), 5);
}

#[test]
fn repair_leaves_payloads_and_timestamps_untouched() {
    let original = vec![
        json!({"event_id": "shared", "type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "chat_response", "message": "answer", "timestamp": "1970-01-01 00:00:01.0"}),
    ];
    let stream = load(original.clone());

    let (_, mut saved) = stream.to_parts().unwrap();
    let mut expected = original;
    for event in saved.iter_mut().chain(&mut expected) {
        event.as_object_mut().unwrap().shift_remove("event_id");
    }
    assert_eq!(saved, expected);
}

#[test]
fn repair_does_not_sanitize_the_stream() {
    // Repair runs before `sanitize`, so a stream that opens without a turn
    // marker still has none afterwards.
    let stream = load(vec![
        json!({"event_id": "shared", "type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
        json!({"event_id": "shared", "type": "chat_response", "message": "answer", "timestamp": "1970-01-01 00:00:01.0"}),
    ]);

    assert_eq!(stream.events.len(), 2);
}

#[test]
fn an_entry_without_an_id_is_assigned_one() {
    let stream = load(vec![json!({"type": "future_event"})]);

    assert!(!ids(&stream)[0].is_empty());
}

#[test]
fn an_entry_with_an_empty_id_is_assigned_one() {
    let stream = load(vec![json!({"type": "future_event", "event_id": ""})]);

    assert!(!ids(&stream)[0].is_empty());
}

#[test]
fn assigned_ids_are_written_on_save() {
    let stream = load(vec![json!({"type": "future_event"})]);

    let (_, saved) = stream.to_parts().unwrap();
    assert_eq!(saved[0]["event_id"], ids(&stream)[0].as_str());
}

#[test]
fn assigning_ids_to_a_legacy_file_reports_no_ambiguity() {
    // An absent ID is unambiguous: no reference to it can exist yet.
    let stream = load(vec![json!({"type": "future_event"})]);

    assert!(stream.duplicated_event_ids().is_empty());
}

#[test]
fn reloading_a_repaired_stream_reports_no_ambiguity() {
    let stream = load(vec![opaque("shared"), opaque("shared")]);
    let (base_config, saved) = stream.to_parts().unwrap();

    let reloaded =
        ConversationStream::from_parts(base_config, saved, &PartialAppConfig::empty()).unwrap();

    assert!(reloaded.duplicated_event_ids().is_empty());
}

#[test]
fn a_repaired_stream_round_trips_unchanged() {
    let stream = load(vec![opaque("shared"), opaque("shared")]);
    let (base_config, saved) = stream.to_parts().unwrap();

    let reloaded =
        ConversationStream::from_parts(base_config, saved.clone(), &PartialAppConfig::empty())
            .unwrap();

    assert_eq!(reloaded.to_parts().unwrap().1, saved);
}

#[test]
fn sanitize_does_not_repair_ids() {
    // Repair belongs to the load path: it reads the IDs a file carried, which
    // is knowledge `sanitize` does not have. `sanitize` repairs stream
    // structure, and a stream carrying duplicate IDs in memory keeps them.
    let mut stream = load(vec![opaque("first"), opaque("second")]);
    stream.events[1].event_id = EventId::fixed("first");

    stream.sanitize();

    assert_eq!(ids(&stream), ["first", "first"]);
    assert!(stream.duplicated_event_ids().is_empty());
}

#[test]
fn ambiguity_is_reported_for_every_duplicated_id() {
    let stream = load(vec![
        opaque("first"),
        opaque("first"),
        opaque("second"),
        opaque("second"),
    ]);

    assert_eq!(
        stream.duplicated_event_ids(),
        &HashSet::from([EventId::fixed("first"), EventId::fixed("second")])
    );
}

#[test]
fn a_hand_edited_id_survives_a_save_and_reload_verbatim() {
    // A hand-edited file can hold anything non-empty, including characters JSON
    // has to escape and an ID long enough to rule out silent truncation. None
    // of it is normalized: what the file holds is what the stream reports and
    // writes back.
    for id in [
        r#"quote" backslash\ brace}"#,
        "newline\nand\ttab",
        "é☃",
        "Hand-edited ID: 42!",
        &"x".repeat(512),
    ] {
        let stream = load(vec![json!({"event_id": id, "type": "future_event"})]);
        assert_eq!(ids(&stream), [id], "loading {id:?}");
        assert!(stream.duplicated_event_ids().is_empty(), "loading {id:?}");

        let (base_config, saved) = stream.to_parts().unwrap();
        assert_eq!(saved[0]["event_id"], id, "saving {id:?}");

        let reloaded =
            ConversationStream::from_parts(base_config, saved, &PartialAppConfig::empty()).unwrap();
        assert_eq!(ids(&reloaded), [id], "reloading {id:?}");
    }
}

#[test]
fn two_entries_sharing_a_hand_edited_id_are_repaired() {
    // Duplicate detection compares the whole string, so an ID needing JSON
    // escaping is matched on what it decodes to rather than on its stored form.
    let id = r#"quote" backslash\"#;
    let stream = load(vec![
        json!({"event_id": id, "type": "future_event"}),
        json!({"event_id": id, "type": "future_event"}),
    ]);

    assert_eq!(ids(&stream)[0], id);
    assert_ne!(ids(&stream)[1], id);
    assert_eq!(
        stream.duplicated_event_ids(),
        &HashSet::from([EventId::fixed(id)])
    );
}

#[test]
fn legacy_events_are_loaded_with_ids_assigned() {
    let (base_config, _) = ConversationStream::new_test().to_parts().unwrap();
    let stream = ConversationStream::from_legacy_events(
        vec![
            json!({"type": "config_delta", "delta": base_config, "timestamp": "1970-01-01 00:00:00.0"}),
            json!({"type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
            json!({"type": "future_event", "content": "opaque"}),
        ],
        &PartialAppConfig::empty(),
    )
    .unwrap()
    .unwrap();

    // The first entry became the base config rather than a stream entry.
    assert_eq!(stream.events.len(), 2);
    assert!(ids(&stream).iter().all(|id| !id.is_empty()));
    assert_ne!(ids(&stream)[0], ids(&stream)[1]);
}

#[test]
fn legacy_events_are_repaired_for_duplicates() {
    let (base_config, _) = ConversationStream::new_test().to_parts().unwrap();
    let stream = ConversationStream::from_legacy_events(
        vec![
            json!({"type": "config_delta", "delta": base_config, "timestamp": "1970-01-01 00:00:00.0"}),
            json!({"event_id": "shared", "type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"}),
            opaque("shared"),
        ],
        &PartialAppConfig::empty(),
    )
    .unwrap()
    .unwrap();

    assert_eq!(ids(&stream)[0], "shared");
    assert_ne!(ids(&stream)[1], "shared");
    assert_eq!(
        stream.duplicated_event_ids(),
        &HashSet::from([EventId::fixed("shared")])
    );
}

#[test]
fn load_history_does_not_affect_stream_equality() {
    // `duplicated_event_ids` and the handed-out ID set are load-scoped. Two
    // streams holding the same entries are equal regardless of whether one of
    // them got there through repair.
    let repaired = load(vec![opaque("shared"), opaque("shared")]);
    let (base_config, saved) = repaired.to_parts().unwrap();
    let clean = ConversationStream::from_parts(base_config, saved, &PartialAppConfig::empty())
        .unwrap()
        .with_created_at(repaired.created_at);

    assert!(!repaired.duplicated_event_ids().is_empty());
    assert!(clean.duplicated_event_ids().is_empty());
    assert_eq!(repaired, clean);
}
