//! Entry identity across the stream's storage, insertion, and view surfaces.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use jp_config::PartialAppConfig;
use serde_json::{Map, Value, from_str, from_value, json, to_string};

use super::{
    ConversationEventWithConfig, ConversationStream, EventPayload, InternalEvent, ResetDelta,
    StoredEvent,
};
use crate::{
    Compaction, ConversationEvent, EventId, SummaryPolicy, ToolCallPolicy,
    event::{ChatRequest, ChatResponse, ToolCallRequest, TurnStart},
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

/// One turn: a marker, a question, and an answer, with known IDs.
fn fixture() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("turn001", event(TurnStart)),
        entry("chat001", event(ChatRequest::from("question"))),
        entry("reply01", event(ChatResponse::message("answer"))),
    ]);
    stream
}

/// A question followed by a tool call that never got a response.
fn orphaned_tool_call() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("chat001", event(ChatRequest::from("question"))),
        entry(
            "tool001",
            event(ToolCallRequest::new(
                "call1".into(),
                "read".into(),
                Map::new(),
            )),
        ),
    ]);
    stream
}

/// The IDs a stream holds, in stream order.
fn ids(stream: &ConversationStream) -> Vec<String> {
    stream
        .events
        .iter()
        .map(|event| event.event_id.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Storage shape
// ---------------------------------------------------------------------------

#[test]
fn a_stored_entry_writes_its_id_before_the_payload() {
    let stored = to_string(&entry("chat001", event(ChatRequest::from("question")))).unwrap();

    assert_eq!(
        stored,
        r#"{"event_id":"chat001","timestamp":"1970-01-01 00:00:00.0","type":"chat_request","content":"question"}"#
    );
}

#[test]
fn every_payload_kind_round_trips_byte_for_byte() {
    // The wrapper's serde is hand-rolled, so each payload kind is pinned to the
    // exact bytes it reads and writes. Base64 fields (`arguments`, `content`,
    // `metadata`) confirm the encode hooks still run under the wrapper.
    for raw in [
        r#"{"event_id":"turn001","timestamp":"1970-01-01 00:00:00.0","type":"turn_start"}"#,
        r#"{"event_id":"chat001","timestamp":"1970-01-01 00:00:00.0","type":"chat_request","content":"question"}"#,
        r#"{"event_id":"reset01","type":"config_delta","op":"reset","timestamp":"1970-01-01 00:00:00.0"}"#,
        r#"{"event_id":"apply01","type":"config_delta","timestamp":"1970-01-01 00:00:00.0","delta":{}}"#,
        r#"{"event_id":"tool001","timestamp":"1970-01-01 00:00:00.0","type":"tool_call_request","id":"tool-id","name":"read","arguments":{"path":"YWJj"},"metadata":{"signature":"YWJj"}}"#,
        r#"{"event_id":"tool002","timestamp":"1970-01-01 00:00:00.0","type":"tool_call_response","id":"tool-id","content":"YWJj","is_error":false}"#,
        r#"{"event_id":"compact","type":"compaction","timestamp":"1970-01-01 00:00:00.0","from_turn":0,"to_turn":1}"#,
        r#"{"event_id":"overlay","type":"event_overlay","timestamp":"1970-01-01 00:00:00.0","patches":[]}"#,
    ] {
        let entry = from_str::<StoredEvent>(raw).unwrap().into_entry();
        assert_eq!(to_string(&entry).unwrap(), raw, "round-tripping {raw}");
    }
}

#[test]
fn an_unknown_entry_round_trips_byte_for_byte() {
    // A newer `jp` may have written a kind this build does not know. Its raw
    // JSON is kept verbatim, so it survives a load and save unchanged.
    let raw = r#"{"event_id":"Future ID!","type":"future_event","id":"payload-id","nested":{"event_id":"not-the-wrapper","bytes":"base64:YWJj"},"extra":[1,null,true]}"#;
    let entry = from_str::<StoredEvent>(raw).unwrap().into_entry();

    assert_eq!(to_string(&entry).unwrap(), raw);
}

#[test]
fn an_unknown_entry_keeps_its_id_on_the_wrapper_only() {
    // The wrapper's `event_id` is lifted out of the stored object, so the
    // payload does not carry a second copy and one key is written back.
    let raw = r#"{"event_id":"Future ID!","type":"future_event","nested":{"event_id":"not-the-wrapper"}}"#;
    let entry = from_str::<StoredEvent>(raw).unwrap().into_entry();

    let EventPayload::Unknown(payload) = &entry.payload else {
        panic!("expected an unknown payload");
    };
    assert_eq!(entry.event_id, EventId::fixed("Future ID!"));
    assert!(payload.get("event_id").is_none());
    // A payload-owned `event_id` deeper in the object is untouched.
    assert_eq!(payload["nested"]["event_id"], "not-the-wrapper");
}

#[test]
fn lifting_the_id_out_leaves_the_other_keys_in_order() {
    // `shift_remove`, not `remove`: a hand-edited entry keeps its key order so
    // a save produces no spurious diff.
    let entry = from_str::<StoredEvent>(
        r#"{"type":"future_event","first":1,"event_id":"middle","last":2}"#,
    )
    .unwrap()
    .into_entry();

    assert_eq!(
        to_string(&entry).unwrap(),
        r#"{"event_id":"middle","type":"future_event","first":1,"last":2}"#
    );
}

#[test]
fn an_entry_that_is_not_an_object_is_rejected() {
    for raw in ["null", "7", r#""a string""#, "[]"] {
        assert!(from_str::<StoredEvent>(raw).is_err(), "accepted {raw}");
    }
}

#[test]
fn an_entry_with_a_non_string_id_is_rejected() {
    // A corrupt ID is not an absent one: it fails loudly rather than being
    // silently replaced. `null` is the exception, and spells absence.
    for id in [json!(1), json!(true), json!([]), json!({})] {
        assert!(
            from_value::<StoredEvent>(json!({"type": "future_event", "event_id": id})).is_err(),
            "accepted {id}"
        );
    }
}

#[test]
fn an_entry_missing_an_id_reads_as_having_none() {
    // Identity belongs to the stream, so the wire form reports what the file
    // held and leaves assigning one to whoever is loading the stream.
    let stored = from_value::<StoredEvent>(json!({"type": "future_event"})).unwrap();

    assert!(stored.event_id.is_none());
}

#[test]
fn an_entry_with_an_empty_id_reads_as_having_none() {
    let stored =
        from_value::<StoredEvent>(json!({"type": "future_event", "event_id": ""})).unwrap();

    assert!(stored.event_id.is_none());
}

#[test]
fn an_entry_with_a_null_id_reads_as_having_none() {
    // `null` is JSON's spelling of "no value", and a hand-edited file is
    // invited to clear an ID. Rejecting it would fail the whole stream load
    // over the one spelling the format itself suggests.
    let stored =
        from_value::<StoredEvent>(json!({"type": "future_event", "event_id": Value::Null}))
            .unwrap();

    assert!(stored.event_id.is_none());
}

// ---------------------------------------------------------------------------
// Identity across a save and load
// ---------------------------------------------------------------------------

#[test]
fn an_id_assigned_to_a_legacy_entry_survives_a_save_and_reload() {
    let (base, _) = fixture().to_parts().unwrap();
    let loaded = ConversationStream::from_parts(
        base,
        vec![json!({
            "type": "chat_request", "content": "question", "timestamp": "1970-01-01 00:00:00.0"
        })],
        &PartialAppConfig::empty(),
    )
    .unwrap();

    let (base, saved) = loaded.to_parts().unwrap();
    let reloaded = ConversationStream::from_parts(base, saved, &PartialAppConfig::empty()).unwrap();

    assert_eq!(
        loaded.first().unwrap().event_id,
        reloaded.first().unwrap().event_id
    );
}

#[test]
fn every_entry_kind_keeps_its_id_across_a_save_and_reload() {
    // The claim this RFD rests on: an entry is addressable by a stable ID
    // whatever its payload, not only when it happens to be a conversation
    // event. A compaction, a patch overlay, a config delta, and an entry this
    // build does not recognize are each as referenceable as a chat request.
    //
    // Only conversation events are reachable through the iteration views, so
    // the others are checked where they *are* addressable: the stored JSON.
    let mut stream = fixture();
    stream.add_config_delta(ResetDelta {
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
    });
    stream.add_compaction(Compaction::new(0, 0));
    stream.add_overlay(Vec::new());
    stream.extend_entries([from_value::<StoredEvent>(
        json!({"event_id": "future1", "type": "future_event"}),
    )
    .unwrap()
    .into_entry()]);
    let before = ids(&stream);

    let (base_config, saved) = stream.to_parts().unwrap();
    let reloaded =
        ConversationStream::from_parts(base_config, saved, &PartialAppConfig::empty()).unwrap();

    assert_eq!(ids(&reloaded), before);
    // Each kind is present, so the assertion above covers all of them rather
    // than a list that happens to be all events.
    let kinds: Vec<_> = reloaded
        .to_parts()
        .unwrap()
        .1
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(kinds, [
        "turn_start",
        "chat_request",
        "chat_response",
        "config_delta",
        "compaction",
        "event_overlay",
        "future_event",
    ]);
}

#[test]
fn editing_an_entry_keeps_its_id() {
    let mut stream = fixture();

    stream
        .iter_mut()
        .nth(1)
        .unwrap()
        .as_chat_request_mut()
        .unwrap()
        .content = "edited".into();

    assert_eq!(ids(&stream)[1], "chat001");
    assert_eq!(
        stream
            .iter()
            .nth(1)
            .unwrap()
            .as_chat_request()
            .unwrap()
            .content,
        "edited"
    );
}

#[test]
fn filtering_the_stream_keeps_the_ids_of_survivors() {
    let mut stream = fixture();

    stream.retain(|event| !event.is_chat_response());

    assert_eq!(ids(&stream), ["turn001", "chat001"]);
}

// ---------------------------------------------------------------------------
// Iteration views
// ---------------------------------------------------------------------------
//
// Every view onto the stream reports the same ID for the same entry. One test
// per view, so a failure names the view that drifted.

#[test]
fn iter_reports_entry_ids() {
    let stream = fixture();

    let seen: Vec<_> = stream
        .iter()
        .map(|event| event.event_id.to_string())
        .collect();

    assert_eq!(seen, ["turn001", "chat001", "reply01"]);
}

#[test]
fn iter_reports_entry_ids_in_reverse() {
    let stream = fixture();

    let seen: Vec<_> = stream
        .iter()
        .rev()
        .map(|event| event.event_id.to_string())
        .collect();

    assert_eq!(seen, ["reply01", "chat001", "turn001"]);
}

#[test]
fn iter_mut_reports_entry_ids() {
    let mut stream = fixture();

    let seen: Vec<_> = stream
        .iter_mut()
        .map(|event| event.event_id.to_string())
        .collect();

    assert_eq!(seen, ["turn001", "chat001", "reply01"]);
}

#[test]
fn into_iter_reports_entry_ids() {
    let seen: Vec<_> = fixture()
        .into_iter()
        .map(|event| event.event_id.to_string())
        .collect();

    assert_eq!(seen, ["turn001", "chat001", "reply01"]);
}

#[test]
fn into_iter_reports_entry_ids_in_reverse() {
    let seen: Vec<_> = fixture()
        .into_iter()
        .rev()
        .map(|event| event.event_id.to_string())
        .collect();

    assert_eq!(seen, ["reply01", "chat001", "turn001"]);
}

#[test]
fn iter_turns_reports_entry_ids() {
    let stream = fixture();

    let seen: Vec<_> = stream
        .iter_turns()
        .flat_map(|turn| {
            turn.iter()
                .map(|event| event.event_id.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert_eq!(seen, ["turn001", "chat001", "reply01"]);
}

#[test]
fn iter_events_by_turn_reports_entry_ids() {
    let stream = fixture();

    let seen: Vec<_> = stream
        .iter_events_by_turn()
        .map(|event| event.event_id.to_string())
        .collect();

    assert_eq!(seen, ["turn001", "chat001", "reply01"]);
}

#[test]
fn iter_events_by_turn_pairs_each_id_with_its_turn() {
    let mut stream = fixture();
    stream.start_turn("next question");

    let seen: Vec<_> = stream
        .iter_events_by_turn()
        .map(|event| (event.turn, event.event_id.to_string()))
        .collect();

    assert_eq!(seen[..3], [
        (0, "turn001".to_owned()),
        (0, "chat001".to_owned()),
        (0, "reply01".to_owned()),
    ]);
    assert_eq!(seen[3].0, 1);
    assert_eq!(seen[4].0, 1);
}

#[test]
fn pop_reports_the_id_of_the_entry_it_removed() {
    let mut stream = fixture();

    assert_eq!(stream.pop().unwrap().event_id, EventId::fixed("reply01"));
}

#[test]
fn converting_a_borrowed_event_keeps_its_id() {
    let stream = fixture();

    let owned = ConversationEventWithConfig::from(stream.first().unwrap());

    assert_eq!(owned.event_id, EventId::fixed("turn001"));
}

#[test]
fn an_unknown_entry_stays_invisible_to_event_iteration() {
    let mut stream = fixture();
    stream.extend_entries([from_value::<StoredEvent>(
        json!({"event_id": "unknown", "type": "future_event", "content": "opaque"}),
    )
    .unwrap()
    .into_entry()]);

    assert_eq!(stream.iter().count(), 3);
    assert_eq!(stream.iter_mut().count(), 3);
    assert_eq!(stream.clone().into_iter().count(), 3);
    assert_eq!(stream.iter_events_by_turn().count(), 3);
    // Invisible to iteration, but still stored and still addressable.
    assert_eq!(stream.to_parts().unwrap().1.len(), 4);
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------

#[test]
fn pushing_an_event_returns_the_id_it_was_given() {
    let mut stream = ConversationStream::new_test();

    let event_id = stream.push_event(event(ChatRequest::from("question")));

    assert_eq!(ids(&stream), [event_id.to_string()]);
}

#[test]
fn starting_a_turn_assigns_ids_to_both_entries() {
    let mut stream = ConversationStream::new_test();

    stream.start_turn("question");

    assert_eq!(ids(&stream).len(), 2);
    assert_ne!(ids(&stream)[0], ids(&stream)[1]);
}

#[test]
fn building_a_turn_assigns_ids_to_its_events() {
    let mut stream = fixture();

    stream
        .current_turn_mut()
        .add_event(event(ChatResponse::message("reply")))
        .build()
        .unwrap();

    assert_eq!(ids(&stream).len(), 4);
    assert!(!ids(&stream)[3].is_empty());
}

#[test]
fn adding_a_config_delta_assigns_it_an_id() {
    let mut stream = ConversationStream::new_test();

    stream.add_config_delta(ResetDelta {
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
    });

    assert!(!ids(&stream)[0].is_empty());
}

#[test]
fn adding_a_compaction_assigns_it_an_id() {
    let mut stream = ConversationStream::new_test();

    stream.add_compaction(Compaction::new(0, 0));

    assert!(!ids(&stream)[0].is_empty());
}

#[test]
fn adding_an_overlay_assigns_it_an_id() {
    let mut stream = ConversationStream::new_test();

    stream.add_overlay(Vec::new());

    assert!(!ids(&stream)[0].is_empty());
}

#[test]
fn every_stored_entry_carries_a_non_empty_id() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    stream.add_config_delta(ResetDelta {
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
    });
    stream.add_compaction(Compaction::new(0, 0));
    stream.add_overlay(Vec::new());

    let (_, saved) = stream.to_parts().unwrap();
    assert!(
        saved
            .iter()
            .all(|event| event["event_id"].as_str().is_some_and(|id| !id.is_empty())),
        "stored entries: {saved:#?}"
    );
}

#[test]
fn trimming_a_turn_keeps_the_id_of_an_overlay_it_preserves() {
    // The overlay is popped and put back, not recreated, so it is the same
    // entry and keeps its identity. Reassigning it would break a reference to
    // an overlay that outlived the turn it happened to sit behind.
    let mut stream = fixture();
    stream.add_compaction(Compaction::new(0, 0));
    let overlay_id = ids(&stream)[3].clone();

    let request = stream.trim_chat_request();

    assert_eq!(request.map(|r| r.content), Some("question".to_owned()));
    assert_eq!(ids(&stream), ["turn001".to_owned(), overlay_id]);
}

#[test]
fn an_id_is_retired_with_the_entry_that_held_it() {
    // The removed entry's ID is never handed out again, so a reference to it
    // fails to resolve rather than binding to a later, unrelated entry.
    let mut stream = ConversationStream::new_test();
    let removed = stream.push_event(event(ChatRequest::from("question")));
    assert_eq!(stream.pop().unwrap().event_id, removed);

    let replacement = stream.push_event(event(ChatRequest::from("another question")));

    assert_ne!(replacement, removed);
}

// ---------------------------------------------------------------------------
// Moving entries between streams
// ---------------------------------------------------------------------------

#[test]
fn appending_a_stream_keeps_the_incoming_ids() {
    // Uniqueness is scoped to one stream, so a copy inherits the source's
    // identities and a reference into the source resolves against the copy.
    let mut destination = ConversationStream::new_test();

    destination.append_stream(fixture());

    assert_eq!(ids(&destination), ["turn001", "chat001", "reply01"]);
}

#[test]
fn appending_a_stream_keeps_the_incoming_payloads() {
    let source = fixture();
    let mut destination = ConversationStream::new_test();

    destination.append_stream(source.clone());

    let payloads: Vec<_> = destination
        .events
        .iter()
        .map(|event| &event.payload)
        .collect();
    let expected: Vec<_> = source.events.iter().map(|event| &event.payload).collect();
    assert_eq!(payloads, expected);
}

#[test]
fn appending_a_stream_replaces_an_id_the_destination_already_holds() {
    // Appending a stream onto itself is the one case where preserving the
    // incoming ID would break uniqueness, so the second copy is reassigned.
    let mut destination = fixture();

    destination.append_stream(fixture());

    assert_eq!(ids(&destination)[..3], ["turn001", "chat001", "reply01"]);
    let unique: HashSet<_> = ids(&destination).into_iter().collect();
    assert_eq!(unique.len(), 6, "every entry holds a distinct ID");
}

#[test]
fn extending_from_another_stream_keeps_the_incoming_ids() {
    let mut destination = ConversationStream::new_test();

    destination.extend(fixture());

    assert_eq!(ids(&destination), ["turn001", "chat001", "reply01"]);
}

#[test]
fn extending_with_bare_events_assigns_fresh_ids() {
    // A `ConversationEvent` carries no identity; the stream entry wrapping it
    // does.
    let mut stream = ConversationStream::new_test();

    stream.extend([event(ChatRequest::from("question"))]);

    assert!(!ids(&stream)[0].is_empty());
}

// ---------------------------------------------------------------------------
// Repair injected by `sanitize`
// ---------------------------------------------------------------------------

#[test]
fn synthetic_entries_are_assigned_distinct_ids() {
    let mut stream = orphaned_tool_call();

    stream.sanitize();

    // A leading `TurnStart` and a synthetic response join the two originals.
    let unique: HashSet<_> = ids(&stream).into_iter().collect();
    assert_eq!(unique.len(), 4);
}

#[test]
fn sanitize_keeps_the_ids_of_the_entries_it_did_not_add() {
    let mut stream = orphaned_tool_call();

    stream.sanitize();

    let seen: Vec<_> = stream
        .iter()
        .map(|event| event.event_id.to_string())
        .collect();
    assert_eq!(seen[1], "chat001");
    assert_eq!(seen[2], "tool001");
}

#[test]
fn a_synthetic_tool_response_keeps_its_requests_timestamp() {
    // Only the ID is stream-assigned; shifting the timestamp to "now" would
    // change ordering semantics.
    let mut stream = orphaned_tool_call();

    stream.sanitize();

    assert!(
        stream
            .iter()
            .all(|event| event.timestamp == DateTime::<Utc>::UNIX_EPOCH)
    );
}

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

/// A stream whose single turn is replaced by a summary when projected.
fn summarized() -> ConversationStream {
    let mut stream = fixture();
    stream.add_compaction(Compaction {
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
        from_turn: 0,
        to_turn: 0,
        summary: Some(SummaryPolicy::generated("summary")),
        reasoning: None,
        tool_calls: None,
    });
    stream
}

#[test]
fn projecting_does_not_touch_the_raw_stream() {
    let raw = summarized();
    let saved = raw.to_parts().unwrap();

    let mut projected = raw.clone();
    projected.apply_projection();

    assert_eq!(raw.to_parts().unwrap(), saved);
}

#[test]
fn a_synthetic_summary_entry_has_no_id_from_the_raw_stream() {
    // Summary entries exist only in the projected view, so their IDs are
    // ephemeral and must not be used as references into `events.json`.
    let raw = summarized();
    let mut projected = raw.clone();

    projected.apply_projection();

    let raw_ids: HashSet<_> = ids(&raw).into_iter().collect();
    assert_eq!(raw_ids.len(), 4, "three entries and the compaction");
    assert!(
        ids(&projected).iter().all(|id| !raw_ids.contains(id)),
        "projected: {:?}, raw: {raw_ids:?}",
        ids(&projected)
    );
}

#[test]
fn synthetic_summary_entries_have_distinct_ids() {
    let mut projected = summarized();

    projected.apply_projection();

    let unique: HashSet<_> = ids(&projected).into_iter().collect();
    assert_eq!(unique.len(), 3);
}

/// Two turns where a compaction summarizes only the first.
fn partially_summarized() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("turn001", event(TurnStart)),
        entry("chat001", event(ChatRequest::from("old question"))),
        entry("reply01", event(ChatResponse::message("old answer"))),
        entry("turn002", event(TurnStart)),
        entry("chat002", event(ChatRequest::from("new question"))),
        entry("reply02", event(ChatResponse::message("new answer"))),
    ]);
    stream.add_compaction(Compaction {
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
        from_turn: 0,
        to_turn: 0,
        summary: Some(SummaryPolicy::generated("summary")),
        reasoning: None,
        tool_calls: None,
    });
    stream
}

#[test]
fn a_retained_entry_keeps_its_id_alongside_an_injected_summary() {
    // The case where both kinds of projected entry coexist: the summary's
    // ephemeral IDs and the second turn's stored IDs are in one list, and the
    // retained entries must still carry the IDs they have in `events.json`.
    let mut projected = partially_summarized();

    projected.apply_projection();

    let ids = ids(&projected);
    assert_eq!(ids.len(), 6, "three synthetic, three retained: {ids:?}");
    assert_eq!(ids[3..], ["turn002", "chat002", "reply02"]);
}

#[test]
fn a_summarized_entrys_id_does_not_reappear_in_the_projected_view() {
    // A reader holding a reference into `events.json` must not find it
    // resolving to a summary entry that stands in for the entry it named.
    //
    // Generated IDs make an accidental reuse vanishingly unlikely, so this is
    // the weaker half of the pair: what makes it hold is that
    // `projection::apply` seeds its `EventIds` from every raw entry before any
    // synthetic one is drawn. `a_draw_retries_past_an_id_the_set_already_holds`
    // pins the drawing itself, deterministically.
    let mut projected = partially_summarized();

    projected.apply_projection();

    let projected_ids = ids(&projected);
    let unique: HashSet<_> = projected_ids.iter().collect();
    assert_eq!(
        unique.len(),
        projected_ids.len(),
        "two projected entries share an ID: {projected_ids:?}"
    );

    let summarized = ["turn001", "chat001", "reply01"];
    assert!(
        projected_ids
            .iter()
            .all(|id| !summarized.contains(&id.as_str())),
        "a synthetic entry took a summarized entry's ID: {projected_ids:?}"
    );
}

#[test]
fn projecting_leaves_the_id_set_covering_the_projected_entries() {
    // `apply_projection` replaces the entry list wholesale, synthetic entries
    // included. The stream's ID set has to come out of that holding every ID
    // its entries carry, or a later insertion can hand out one of them again.
    let mut projected = partially_summarized();

    projected.apply_projection();

    assert!(projected.id_set_covers_entries());
}

#[test]
fn a_cloned_stream_carries_the_ids_its_source_had_handed_out() {
    // `PartialEq` deliberately ignores `event_ids`, so a clone that lost the
    // set still compares equal to its source and most tests would not notice.
    // `append_stream(self.clone())` would: the copy would see no collisions and
    // keep every incoming ID, leaving the result holding each one twice.
    // The pop retires an ID, so the set holds one the entries no longer carry
    // and a clone that rebuilt it from the entries would come out different.
    let mut source = fixture();
    assert!(source.pop().is_some());

    let mut clone = source.clone();
    clone.append_stream(source.clone());

    assert!(clone.id_set_covers_entries());
    let unique: HashSet<_> = ids(&clone).into_iter().collect();
    assert_eq!(unique.len(), ids(&clone).len(), "{:?}", ids(&clone));
}

#[test]
fn a_stream_that_was_never_projected_covers_its_entries() {
    // The same invariant on the ordinary paths, so a failure above points at
    // projection rather than at insertion.
    let mut stream = fixture();
    stream.start_turn("question");
    stream.add_compaction(Compaction::new(0, 0));
    stream.append_stream(fixture());
    stream.sanitize();

    assert!(stream.id_set_covers_entries());
}

/// A stream whose tool call request is blanked, not removed, when projected.
fn stripped_tool_call() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.extend_entries([
        entry("turn001", event(TurnStart)),
        entry("chat001", event(ChatRequest::from("question"))),
        entry(
            "tool001",
            event(ToolCallRequest::new(
                "call1".into(),
                "read".into(),
                Map::from_iter([("path".into(), json!("src/main.rs"))]),
            )),
        ),
    ]);
    stream.add_compaction(Compaction {
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
        from_turn: 0,
        to_turn: 0,
        summary: None,
        reasoning: None,
        tool_calls: Some(
            ToolCallPolicy::Strip {
                request: true,
                response: false,
            }
            .into(),
        ),
    });
    stream
}

#[test]
fn a_mechanically_projected_entry_keeps_its_raw_id() {
    // A policy that rewrites an entry's content leaves its identity alone: the
    // projected entry still points at the raw entry it came from.
    let mut projected = stripped_tool_call();

    projected.apply_projection();

    assert_eq!(ids(&projected), ["turn001", "chat001", "tool001"]);
}

#[test]
fn a_mechanically_projected_entry_has_its_content_rewritten() {
    let mut projected = stripped_tool_call();

    projected.apply_projection();

    assert!(
        projected
            .last()
            .unwrap()
            .as_tool_call_request()
            .unwrap()
            .arguments
            .is_empty()
    );
}
