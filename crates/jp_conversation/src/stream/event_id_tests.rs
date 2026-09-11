use std::collections::HashSet;

use chrono::{DateTime, Utc};
use jp_config::PartialAppConfig;
use serde_json::{Map, Value, from_str, from_value, json, to_string};

use super::{
    ConversationEventWithConfig, ConversationStream, EventPayload, InternalEvent, ResetDelta,
};
use crate::{
    Compaction, ConversationEvent, EventId, SummaryPolicy, ToolCallPolicy,
    event::{ChatRequest, ChatResponse, ToolCallRequest, TurnStart},
};

fn entry(id: &str, payload: EventPayload) -> InternalEvent {
    InternalEvent {
        event_id: EventId::fixed(id),
        payload,
    }
}

fn fixture() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.events = vec![
        entry(
            "turn001",
            EventPayload::Event(Box::new(ConversationEvent::new(
                TurnStart,
                DateTime::<Utc>::UNIX_EPOCH,
            ))),
        ),
        entry(
            "chat001",
            EventPayload::Event(Box::new(ConversationEvent::new(
                ChatRequest::from("question"),
                DateTime::<Utc>::UNIX_EPOCH,
            ))),
        ),
        entry(
            "reply01",
            EventPayload::Event(Box::new(ConversationEvent::new(
                ChatResponse::message("answer"),
                DateTime::<Utc>::UNIX_EPOCH,
            ))),
        ),
    ];
    stream
}

#[test]
fn known_payloads_roundtrip_with_exact_storage_bytes() {
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
        let event: InternalEvent = from_str(raw).unwrap();
        assert_eq!(to_string(&event).unwrap(), raw);
    }
}

#[test]
fn unknown_payload_roundtrips_with_exact_storage_bytes() {
    let raw = r#"{"event_id":"Future ID!","type":"future_event","id":"payload-id","nested":{"event_id":"not-the-wrapper","bytes":"base64:YWJj"},"extra":[1,null,true]}"#;
    let event: InternalEvent = from_str(raw).unwrap();
    let EventPayload::Unknown(payload) = &event.payload else {
        panic!("unknown payload")
    };

    assert!(payload.get("event_id").is_none());
    assert_eq!(event.event_id, EventId::fixed("Future ID!"));
    assert_eq!(to_string(&event).unwrap(), raw);
}

#[test]
fn extracting_id_preserves_unknown_payload_field_order() {
    let raw = r#"{"type":"future_event","first":1,"event_id":"middle","last":2}"#;
    let event: InternalEvent = from_str(raw).unwrap();

    assert_eq!(
        to_string(&event).unwrap(),
        r#"{"event_id":"middle","type":"future_event","first":1,"last":2}"#
    );
}

#[test]
fn missing_and_empty_ids_are_assigned_at_load() {
    let missing: InternalEvent = from_value(json!({"type": "future_event"})).unwrap();
    let empty: InternalEvent = from_value(json!({"type": "future_event", "event_id": ""})).unwrap();

    assert!(!missing.event_id.to_string().is_empty());
    assert!(!empty.event_id.to_string().is_empty());
    assert_ne!(missing.event_id, empty.event_id);
}

#[test]
fn non_string_ids_are_rejected() {
    for id in [Value::Null, json!(1), json!(true), json!([]), json!({})] {
        assert!(
            from_value::<InternalEvent>(json!({"type": "future_event", "event_id": id})).is_err()
        );
    }
}

#[test]
fn legacy_entries_keep_assigned_ids_after_save_and_reload() {
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
    let reloaded =
        ConversationStream::from_parts(base, saved.clone(), &PartialAppConfig::empty()).unwrap();

    assert_eq!(reloaded.to_parts().unwrap().1, saved);
    assert_eq!(
        loaded.first().unwrap().event_id,
        reloaded.first().unwrap().event_id
    );
}

#[test]
fn iteration_views_expose_the_same_ids() {
    let mut stream = fixture();
    assert_eq!(stream.first().unwrap().event_id, &EventId::fixed("turn001"));
    assert_eq!(
        stream.iter().next_back().unwrap().event_id,
        &EventId::fixed("reply01")
    );
    assert_eq!(
        stream
            .iter_turns()
            .next()
            .unwrap()
            .iter()
            .nth(1)
            .unwrap()
            .event_id,
        &EventId::fixed("chat001")
    );
    assert_eq!(
        stream.iter_events_by_turn().nth(1).unwrap().1,
        &EventId::fixed("chat001")
    );
    assert_eq!(
        stream.iter_mut().nth(1).unwrap().event_id,
        &EventId::fixed("chat001")
    );
    assert_eq!(
        stream.clone().into_iter().next().unwrap().event_id,
        EventId::fixed("turn001")
    );
    assert_eq!(
        stream.clone().into_iter().next_back().unwrap().event_id,
        EventId::fixed("reply01")
    );
    assert_eq!(
        ConversationEventWithConfig::from(stream.first().unwrap()).event_id,
        EventId::fixed("turn001")
    );
    assert_eq!(stream.pop().unwrap().event_id, EventId::fixed("reply01"));
}

#[test]
fn editing_and_filtering_preserve_surviving_ids() {
    let mut stream = fixture();
    stream
        .iter_mut()
        .nth(1)
        .unwrap()
        .as_chat_request_mut()
        .unwrap()
        .content = "edited".into();
    stream.retain(|event| !event.is_chat_response());
    stream.sanitize();

    assert_eq!(stream.first().unwrap().event_id, &EventId::fixed("turn001"));
    assert_eq!(stream.last().unwrap().event_id, &EventId::fixed("chat001"));
    assert_eq!(
        stream.last().unwrap().as_chat_request().unwrap().content,
        "edited"
    );
}

#[test]
fn insertion_paths_assign_ids_to_every_payload() {
    let mut stream = fixture();
    stream.start_turn("next");
    stream
        .current_turn_mut()
        .add_event(ConversationEvent::new(
            ChatResponse::message("reply"),
            DateTime::<Utc>::UNIX_EPOCH,
        ))
        .build()
        .unwrap();
    stream.add_config_delta(ResetDelta {
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
    });
    stream.add_compaction(Compaction::new(0, 0));
    stream.add_overlay(Vec::new());
    stream.extend([ConversationEvent::new(
        ChatResponse::message("more"),
        DateTime::<Utc>::UNIX_EPOCH,
    )]);
    stream.extend(fixture());

    let ids: HashSet<_> = stream.events.iter().map(|event| &event.event_id).collect();
    assert_eq!(ids.len(), stream.events.len());
    assert!(
        stream
            .to_parts()
            .unwrap()
            .1
            .iter()
            .all(|event| event["event_id"].as_str().is_some_and(|id| !id.is_empty()))
    );
}

#[test]
fn append_stream_assigns_new_ids_without_changing_payloads() {
    let mut stream = fixture();
    let source = fixture();
    stream.append_stream(source.clone());

    assert_eq!(stream.events.len(), 6);
    assert_eq!(stream.events[3].payload, source.events[0].payload);
    assert_eq!(stream.events[4].payload, source.events[1].payload);
    assert_eq!(stream.events[5].payload, source.events[2].payload);
    let ids: HashSet<_> = stream.events.iter().map(|event| &event.event_id).collect();
    assert_eq!(ids.len(), 6);
}

#[test]
fn synthetic_repairs_assign_ids_and_preserve_source_timestamps() {
    let mut stream = ConversationStream::new_test();
    stream.events = vec![
        entry(
            "chat001",
            EventPayload::Event(Box::new(ConversationEvent::new(
                ChatRequest::from("question"),
                DateTime::<Utc>::UNIX_EPOCH,
            ))),
        ),
        entry(
            "tool001",
            EventPayload::Event(Box::new(ConversationEvent::new(
                ToolCallRequest::new("call1".into(), "read".into(), Map::new()),
                DateTime::<Utc>::UNIX_EPOCH,
            ))),
        ),
    ];
    stream.sanitize();

    assert_eq!(stream.len(), 4);
    assert!(
        stream
            .iter()
            .all(|event| event.timestamp == DateTime::<Utc>::UNIX_EPOCH)
    );
    assert_eq!(
        stream.iter().nth(1).unwrap().event_id,
        &EventId::fixed("chat001")
    );
    assert_eq!(
        stream.iter().nth(2).unwrap().event_id,
        &EventId::fixed("tool001")
    );
    let ids: HashSet<_> = stream.events.iter().map(|event| &event.event_id).collect();
    assert_eq!(ids.len(), 4);
}

#[test]
fn summary_projection_assigns_ephemeral_ids_without_modifying_raw_stream() {
    let mut raw = fixture();
    raw.add_compaction(Compaction {
        timestamp: DateTime::<Utc>::UNIX_EPOCH,
        from_turn: 0,
        to_turn: 0,
        summary: Some(SummaryPolicy::generated("summary")),
        reasoning: None,
        tool_calls: None,
    });
    let saved = raw.to_parts().unwrap();
    let mut projected = raw.clone();
    projected.apply_projection();

    assert_eq!(raw.to_parts().unwrap(), saved);
    assert_eq!(projected.len(), 3);
    assert!(
        projected
            .iter()
            .all(|event| event.timestamp == DateTime::<Utc>::UNIX_EPOCH)
    );
    assert!(
        projected
            .events
            .iter()
            .all(|event| raw.events.iter().all(|raw| raw.event_id != event.event_id))
    );
    let ids: HashSet<_> = projected
        .events
        .iter()
        .map(|event| &event.event_id)
        .collect();
    assert_eq!(ids.len(), 3);
}

#[test]
fn mechanical_projection_preserves_ids_of_retained_entries() {
    let mut raw = fixture();
    raw.events.push(entry(
        "tool001",
        EventPayload::Event(Box::new(ConversationEvent::new(
            ToolCallRequest::new(
                "call1".into(),
                "read".into(),
                Map::from_iter([("path".into(), json!("src/main.rs"))]),
            ),
            DateTime::<Utc>::UNIX_EPOCH,
        ))),
    ));
    raw.add_compaction(Compaction {
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
    let saved = raw.to_parts().unwrap();
    let mut projected = raw.clone();
    projected.apply_projection();

    let ids: Vec<_> = projected
        .iter()
        .map(|event| event.event_id.to_string())
        .collect();
    assert_eq!(ids, ["turn001", "chat001", "reply01", "tool001"]);
    assert!(
        projected
            .last()
            .unwrap()
            .as_tool_call_request()
            .unwrap()
            .arguments
            .is_empty()
    );
    assert_eq!(raw.to_parts().unwrap(), saved);
}

#[test]
fn unknown_entries_remain_invisible_to_event_iteration() {
    let mut stream = fixture();
    stream.events.push(
        from_value(
            json!({"event_id": "unknown", "type": "future_event", "content": "not for providers"}),
        )
        .unwrap(),
    );
    assert_eq!(stream.iter().count(), 3);
    assert_eq!(stream.iter_mut().count(), 3);
    assert_eq!(stream.clone().into_iter().count(), 3);
    assert_eq!(stream.iter_events_by_turn().count(), 3);
    assert_eq!(stream.to_parts().unwrap().1.len(), 4);
}
