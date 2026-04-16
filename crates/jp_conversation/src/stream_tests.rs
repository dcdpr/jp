use chrono::TimeZone as _;
use jp_config::{
    PartialConfig as _,
    conversation::tool::{PartialToolConfig, RunMode},
};
use serde_json::{Map, Value};

use super::*;
use crate::{
    Compaction, CompactionRange, RangeBound, ReasoningPolicy, ToolCallPolicy,
    event::{
        ChatResponse, InquiryQuestion, InquiryRequest, InquiryResponse, InquirySource,
        ToolCallRequest,
    },
    resolve_range,
};

#[test]
fn test_sanitize_orphaned_tool_calls_injects_directly_after_request() {
    let mut stream = ConversationStream::new_test();

    stream.push(ChatRequest::from("Hello"));
    stream.push(ToolCallRequest {
        id: "orphan_1".into(),
        name: "some_tool".into(),
        arguments: Map::new(),
    });
    stream.push(ChatResponse::message("trailing"));

    stream.sanitize_orphaned_tool_calls();

    // Verify the response was injected.
    let response = stream.find_tool_call_response("orphan_1");
    assert!(response.is_some(), "Expected synthetic response for orphan");
    assert!(response.unwrap().result.is_err());
    assert!(response.unwrap().content().contains("interrupted"));

    // Verify ordering: request must be immediately followed by its response -
    // no events in between.
    let events: Vec<_> = stream.iter().collect();
    let req_pos = events
        .iter()
        .position(|e| {
            e.event
                .as_tool_call_request()
                .is_some_and(|r| r.id == "orphan_1")
        })
        .unwrap();
    let resp_pos = events
        .iter()
        .position(|e| {
            e.event
                .as_tool_call_response()
                .is_some_and(|r| r.id == "orphan_1")
        })
        .unwrap();

    assert_eq!(
        resp_pos,
        req_pos + 1,
        "Response must be directly after request"
    );
}

#[test]
fn test_sanitize_orphaned_tool_calls_leaves_matched_alone() {
    let mut stream = ConversationStream::new_test();

    stream.push(ToolCallRequest {
        id: "matched_1".into(),
        name: "tool".into(),
        arguments: Map::new(),
    });
    stream.push(ToolCallResponse {
        id: "matched_1".into(),
        result: Ok("ok".into()),
    });

    let len_before = stream.len();
    stream.sanitize_orphaned_tool_calls();
    assert_eq!(stream.len(), len_before, "No extra events should be added");
}

#[test]
fn test_sanitize_orphaned_tool_calls_handles_partial_set() {
    let mut stream = ConversationStream::new_test();

    // Two parallel requests, only 'a' has a response.
    stream.push(ToolCallRequest {
        id: "a".into(),
        name: "tool".into(),
        arguments: Map::new(),
    });
    stream.push(ToolCallRequest {
        id: "b".into(),
        name: "tool".into(),
        arguments: Map::new(),
    });
    stream.push(ToolCallResponse {
        id: "a".into(),
        result: Ok("ok".into()),
    });

    stream.sanitize_orphaned_tool_calls();

    // 'b' should get a synthetic response directly after its request.
    let events: Vec<_> = stream.iter().collect();
    let req_b = events
        .iter()
        .position(|e| e.event.as_tool_call_request().is_some_and(|r| r.id == "b"))
        .unwrap();
    let resp_b = events
        .iter()
        .position(|e| e.event.as_tool_call_response().is_some_and(|r| r.id == "b"))
        .unwrap();
    assert_eq!(
        resp_b,
        req_b + 1,
        "Synthetic response for 'b' must follow its request"
    );

    // 'a' should still have its original response.
    assert_eq!(stream.find_tool_call_response("a").unwrap().content(), "ok");
}

#[test]
fn test_trim_trailing_empty_turn_removes_lone_turn_start() {
    let mut stream = ConversationStream::new_test();
    stream.push(TurnStart);
    assert_eq!(stream.len(), 1);

    stream.trim_trailing_empty_turn();
    assert_eq!(stream.len(), 0);
}

#[test]
fn test_trim_trailing_empty_turn_keeps_non_empty_turn() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("Hello");
    assert_eq!(stream.len(), 2);

    stream.trim_trailing_empty_turn();
    assert_eq!(stream.len(), 2, "Turn with events should not be removed");
}

#[test]
fn sanitize_removes_trailing_empty_turn_after_popped_chat_request() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    // Simulate interrupted turn: pop the ChatRequest, leaving an orphaned TurnStart.
    let popped = stream.pop_if(ConversationEvent::is_chat_request);
    assert!(popped.is_some());
    assert_eq!(stream.len(), 1, "Only TurnStart should remain");

    // Sanitize should clean up the orphaned TurnStart.
    stream.sanitize();
    assert_eq!(stream.len(), 0, "Orphaned TurnStart should be removed");

    // Re-adding a turn should produce exactly one TurnStart + ChatRequest.
    stream.start_turn("hello again");
    let turn_starts = stream.iter().filter(|e| e.event.is_turn_start()).count();
    assert_eq!(turn_starts, 1, "Should have exactly one TurnStart");
}

/// Regression test: when earlier turns exist, sanitize still removes an
/// orphaned trailing `TurnStart` left after popping the last `ChatRequest`.
#[test]
fn sanitize_removes_trailing_empty_turn_with_prior_turns() {
    let mut stream = ConversationStream::new_test();

    // First turn: complete (has a response).
    stream.start_turn("first");
    stream
        .current_turn_mut()
        .add_event(ChatResponse::message("reply"))
        .build()
        .unwrap();

    // Second turn: interrupted — only TurnStart + ChatRequest.
    stream.start_turn("second");
    assert_eq!(stream.len(), 5); // TS + CR + Resp + TS + CR

    // Pop the ChatRequest from the interrupted turn.
    let popped = stream.pop_if(ConversationEvent::is_chat_request);
    assert!(popped.is_some());
    assert_eq!(stream.len(), 4); // TS + CR + Resp + TS

    // Sanitize should remove the trailing orphaned TurnStart.
    stream.sanitize();

    let turn_starts = stream.iter().filter(|e| e.event.is_turn_start()).count();
    assert_eq!(
        turn_starts, 1,
        "Only the first turn's TurnStart should remain"
    );
    assert_eq!(stream.len(), 3); // TS + CR + Resp
}

#[test]
fn test_to_parts_from_parts_roundtrip() {
    let mut stream = ConversationStream::new_test();

    // Empty stream roundtrips.
    let (base_config, events) = stream.to_parts().unwrap();
    assert!(events.is_empty());
    let stream2 = ConversationStream::from_parts(base_config, events)
        .unwrap()
        .with_created_at(stream.created_at);
    assert_eq!(stream, stream2);

    // Add some events and roundtrip again.
    stream
        .events
        .push(InternalEvent::Event(Box::new(ConversationEvent::new(
            ChatRequest::from("foo"),
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        ))));

    stream
        .events
        .push(InternalEvent::Event(Box::new(ConversationEvent::new(
            ChatResponse::message("bar"),
            Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap(),
        ))));

    let (base_config, events) = stream.to_parts().unwrap();
    assert_eq!(events.len(), 2);
    let stream3 = ConversationStream::from_parts(base_config, events)
        .unwrap()
        .with_created_at(stream.created_at);
    assert_eq!(stream, stream3);
}

#[test]
fn test_from_parts_strips_unknown_base_config_fields() {
    let stream = ConversationStream::new_test();
    let (mut base_config, events) = stream.to_parts().unwrap();

    // Inject unknown fields into the base config JSON, simulating a schema
    // change where a field was removed.
    let obj = base_config.as_object_mut().unwrap();
    obj.insert("removed_field".into(), Value::String("stale".into()));

    // from_parts should strip the unknown field and load successfully.
    let result = ConversationStream::from_parts(base_config, events);
    assert!(
        result.is_ok(),
        "from_parts should tolerate unknown fields in base config"
    );
}

#[test]
fn test_to_parts_base64_encodes_tool_call_fields() {
    let mut stream = ConversationStream::new_test();

    let mut args = Map::new();
    args.insert("path".into(), Value::String("src/main.rs".into()));

    stream.push(ToolCallRequest {
        id: "tc1".into(),
        name: "read_file".into(),
        arguments: args,
    });
    stream.push(ToolCallResponse {
        id: "tc1".into(),
        result: Ok("file contents here".into()),
    });

    // Serialize via to_parts (as storage would).
    let (_config, events) = stream.to_parts().unwrap();
    let json = serde_json::to_string(&events).unwrap();

    // The raw JSON should NOT contain the plain-text values — they
    // should be base64-encoded.
    assert!(
        !json.contains("src/main.rs"),
        "Tool arguments should be base64-encoded on disk"
    );
    assert!(
        !json.contains("file contents here"),
        "Tool response content should be base64-encoded on disk"
    );

    // Roundtrip via from_parts should recover the original values.
    let base_config = serde_json::to_value(stream.base_config().to_partial()).unwrap();
    let stream2 = ConversationStream::from_parts(base_config, events).unwrap();

    let req = stream2
        .iter()
        .find_map(|e| e.event.as_tool_call_request())
        .unwrap();
    assert_eq!(req.arguments["path"], "src/main.rs");

    let resp = stream2.find_tool_call_response("tc1").unwrap();
    assert_eq!(resp.content(), "file contents here");
}

#[test]
fn test_sanitize_drops_leading_non_user_events() {
    let mut stream = ConversationStream::new_test();

    // Simulate a fork where --from cut into the middle of a turn:
    // the ChatRequest was removed but the response and subsequent
    // events remain.
    stream.push(ConversationEvent::new(
        ChatResponse::message("orphaned response"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatRequest::from("real question"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("real answer"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 2).unwrap(),
    ));

    stream.sanitize();

    // TurnStart injected + ChatRequest + ChatResponse
    assert_eq!(stream.len(), 3);
    assert!(
        stream.first().unwrap().event.is_turn_start(),
        "Stream should start with TurnStart after sanitize"
    );
}

#[test]
fn test_sanitize_removes_orphaned_tool_call_response() {
    let mut stream = ConversationStream::new_test();

    stream.push(ConversationEvent::new(
        ChatRequest::from("question"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ToolCallResponse {
            id: "orphan".into(),
            result: Ok("data".into()),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("answer"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 2).unwrap(),
    ));

    stream.sanitize();

    // TurnStart injected + ChatRequest + ChatResponse
    assert_eq!(stream.len(), 3);
    assert!(
        stream.find_tool_call_response("orphan").is_none(),
        "Orphaned ToolCallResponse should be removed"
    );
}

#[test]
fn test_sanitize_removes_orphaned_inquiry_response() {
    let mut stream = ConversationStream::new_test();

    stream.push(ConversationEvent::new(
        ChatRequest::from("question"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        InquiryResponse::boolean("orphan", true),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("answer"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 2).unwrap(),
    ));

    stream.sanitize();

    // TurnStart injected + ChatRequest + ChatResponse
    assert_eq!(stream.len(), 3);
    let has_inquiry_response = stream.iter().any(|e| e.event.is_inquiry_response());
    assert!(
        !has_inquiry_response,
        "Orphaned InquiryResponse should be removed"
    );
}

#[test]
fn test_sanitize_removes_orphaned_inquiry_request() {
    let mut stream = ConversationStream::new_test();

    stream.push(ConversationEvent::new(
        ChatRequest::from("question"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        InquiryRequest::new(
            "orphan",
            InquirySource::Assistant,
            InquiryQuestion::boolean("proceed?".into()),
        ),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("answer"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 2).unwrap(),
    ));

    stream.sanitize();

    // TurnStart injected + ChatRequest + ChatResponse
    assert_eq!(stream.len(), 3);
    let has_inquiry_request = stream.iter().any(|e| e.event.is_inquiry_request());
    assert!(
        !has_inquiry_request,
        "Orphaned InquiryRequest should be removed"
    );
}

#[test]
fn test_sanitize_keeps_matched_pairs_intact() {
    let mut stream = ConversationStream::new_test();

    stream.push(ConversationEvent::new(
        ChatRequest::from("question"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ToolCallRequest {
            id: "tc1".into(),
            name: "read_file".into(),
            arguments: Map::new(),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ToolCallResponse {
            id: "tc1".into(),
            result: Ok("contents".into()),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 2).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        InquiryRequest::new(
            "iq1",
            InquirySource::tool("read_file"),
            InquiryQuestion::boolean("proceed?".into()),
        ),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 3).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        InquiryResponse::boolean("iq1", true),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 4).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("done"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 5).unwrap(),
    ));

    stream.sanitize();

    // TurnStart injected + 6 original events
    assert_eq!(
        stream.len(),
        7,
        "All matched events should be preserved (plus injected TurnStart)"
    );
}

#[test]
fn test_sanitize_handles_from_cutting_through_tool_call() {
    let mut stream = ConversationStream::new_test();

    // Simulates --from removing the ToolCallRequest but keeping
    // its response, plus the subsequent turn.
    stream.push(ConversationEvent::new(
        ToolCallResponse {
            id: "cut".into(),
            result: Ok("data".into()),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("based on that tool..."),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 2).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatRequest::from("next question"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 3).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("next answer"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 4).unwrap(),
    ));

    stream.sanitize();

    // Leading ToolCallResponse and ChatResponse dropped,
    // orphaned ToolCallResponse also removed.
    // TurnStart injected + ChatRequest + ChatResponse
    assert_eq!(stream.len(), 3);
    assert!(stream.first().unwrap().event.is_turn_start());
}

#[test]
fn test_sanitize_deduplicates_leading_turn_starts() {
    let mut stream = ConversationStream::new_test();

    // Simulate --from keeping TurnStarts from multiple filtered turns
    // that all precede the first ChatRequest.
    stream.push(TurnStart);
    stream.push(TurnStart);
    stream.push(TurnStart);
    stream.push(ConversationEvent::new(
        ChatRequest::from("hello"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("hi"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));

    stream.sanitize();

    let turn_count = stream.iter().filter(|e| e.event.is_turn_start()).count();
    assert_eq!(turn_count, 1, "Should have exactly one TurnStart");
}

#[test]
fn test_sanitize_reindexes_turn_starts() {
    let mut stream = ConversationStream::new_test();

    // Two turns with non-sequential indices (from filtering).
    stream.push(TurnStart);
    stream.push(ConversationEvent::new(
        ChatRequest::from("first"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("reply"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.push(TurnStart);
    stream.push(ConversationEvent::new(
        ChatRequest::from("second"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 2).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("reply2"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 3).unwrap(),
    ));

    stream.sanitize();

    let turn_count = stream.iter().filter(|e| e.event.is_turn_start()).count();
    assert_eq!(turn_count, 2);
}

#[test]
/// When no `ChatRequest` exists, `sanitize` preserves the events (useful for
/// fork/storage scenarios where a user will add a `ChatRequest` later).
/// Provider-bound code must handle this separately.
fn test_sanitize_preserves_events_when_no_chat_request() {
    let mut stream = ConversationStream::new_test();

    stream.push(ConversationEvent::new(
        ChatResponse::reasoning("Thinking about it..."),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ToolCallRequest {
            id: "tc1".into(),
            name: "git_stage_patch".into(),
            arguments: Map::new(),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ToolCallResponse {
            id: "tc1".into(),
            result: Ok("Tool paused: confirm?".into()),
        },
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 2).unwrap(),
    ));

    stream.sanitize();

    // Events are preserved (TurnStart injected + 3 original events).
    // sanitize does NOT drop content when there's no ChatRequest — that
    // is the caller's responsibility when sending to a provider.
    assert_eq!(stream.len(), 4);
    assert!(stream.first().unwrap().event.is_turn_start());
}

#[test]
fn test_has_chat_request() {
    let empty = ConversationStream::new_test();
    assert!(!empty.has_chat_request());

    let with_request = ConversationStream::new_test().with_turn("hello");
    assert!(with_request.has_chat_request());

    let mut no_request = ConversationStream::new_test();
    no_request.push(ConversationEvent::new(
        ChatResponse::message("orphaned"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    assert!(!no_request.has_chat_request());
}

#[test]
fn test_schema_returns_none_when_no_schema() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));

    assert!(stream.schema().is_none());
}

#[test]
fn test_schema_from_initial_chat_request() {
    let schema = Map::from_iter([("type".into(), Value::String("object".into()))]);
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest {
        content: "query".into(),
        schema: Some(schema.clone()),
    });

    assert_eq!(stream.schema(), Some(schema));
}

#[test]
fn test_schema_survives_tool_use_round_trip() {
    let schema = Map::from_iter([("type".into(), Value::String("object".into()))]);
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest {
        content: "query".into(),
        schema: Some(schema.clone()),
    });

    stream
        .current_turn_mut()
        .add_tool_call_request(ToolCallRequest {
            id: "tc1".into(),
            name: "my_tool".into(),
            arguments: Map::new(),
        })
        .add_tool_call_response(ToolCallResponse {
            id: "tc1".into(),
            result: Ok("done".into()),
        })
        .build()
        .unwrap();

    assert_eq!(stream.schema(), Some(schema));
}

#[test]
fn test_schema_not_inherited_from_previous_turn() {
    let schema = Map::from_iter([("type".into(), Value::String("object".into()))]);
    let mut stream = ConversationStream::new_test();

    // First turn has a schema.
    stream.start_turn(ChatRequest {
        content: "structured query".into(),
        schema: Some(schema),
    });
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("response"))
        .build()
        .unwrap();

    // Second turn has no schema.
    stream.start_turn(ChatRequest::from("plain query"));

    assert!(stream.schema().is_none());
}

#[test]
fn test_schema_ignores_interrupt_reply() {
    let schema = Map::from_iter([("type".into(), Value::String("object".into()))]);
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest {
        content: "query".into(),
        schema: Some(schema.clone()),
    });

    // Simulate an interrupt reply (schema: None).
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("partial"))
        .add_chat_request(ChatRequest {
            content: "continue".into(),
            schema: None,
        })
        .build()
        .unwrap();

    // Should still find the original schema, not the interrupt's None.
    assert_eq!(stream.schema(), Some(schema));
}

#[test]
fn test_sanitize_noop_on_healthy_stream() {
    let mut stream = ConversationStream::new_test();

    stream.push(TurnStart);
    stream.push(ConversationEvent::new(
        ChatRequest::from("hello"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("hi"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));

    let len_before = stream.len();
    stream.sanitize();
    assert_eq!(
        stream.len(),
        len_before,
        "Healthy stream should be unchanged"
    );
}

// --- InternalEvent config delta roundtrip tests ---

/// Serialize a [`ConfigDelta`] as an [`InternalEvent`] and deserialize it back.
fn roundtrip_delta(delta: ConfigDelta) -> ConfigDelta {
    let event = InternalEvent::ConfigDelta(delta);
    let json = serde_json::to_value(&event).unwrap();
    let deserialized: InternalEvent = serde_json::from_value(json).unwrap();
    match deserialized {
        InternalEvent::ConfigDelta(d) => d,
        _ => panic!("expected ConfigDelta"),
    }
}

#[test]
fn test_roundtrip_default_config_preserves_all_fields() {
    let original = ConfigDelta::from(jp_config::AppConfig::new_test().to_partial());
    let result = roundtrip_delta(original.clone());

    let original_json = serde_json::to_value(&original).unwrap();
    let result_json = serde_json::to_value(&result).unwrap();
    assert_eq!(original_json, result_json);
}

#[test]
fn test_roundtrip_empty_delta() {
    let original = ConfigDelta::from(jp_config::PartialAppConfig::empty());
    let result = roundtrip_delta(original.clone());
    assert_eq!(original, result);
}

#[test]
fn test_roundtrip_delta_with_tool_defaults() {
    let mut partial = jp_config::PartialAppConfig::empty();
    partial.conversation.tools.defaults.run = Some(RunMode::Unattended);

    let original = ConfigDelta::from(partial);
    let result = roundtrip_delta(original.clone());

    let original_json = serde_json::to_value(&original).unwrap();
    let result_json = serde_json::to_value(&result).unwrap();
    assert_eq!(original_json, result_json);
}

/// Per-tool entries are serialized as flattened keys alongside "*" in the
/// tools object. The schema only knows about "*", so a naive strip would
/// remove all per-tool config.
#[test]
fn test_roundtrip_delta_with_per_tool_overrides() {
    let mut partial = jp_config::PartialAppConfig::empty();
    partial.conversation.tools.defaults.run = Some(RunMode::Ask);

    partial
        .conversation
        .tools
        .tools
        .insert("fs_read_file".into(), PartialToolConfig {
            run: Some(RunMode::Unattended),
            ..Default::default()
        });
    partial
        .conversation
        .tools
        .tools
        .insert("cargo_check".into(), PartialToolConfig {
            run: Some(RunMode::Unattended),
            ..Default::default()
        });

    let original = ConfigDelta::from(partial);
    let result = roundtrip_delta(original.clone());

    let original_json = serde_json::to_value(&original).unwrap();
    let result_json = serde_json::to_value(&result).unwrap();
    assert_eq!(original_json, result_json);
}

#[test]
fn test_roundtrip_delta_strip_unknown_field_preserves_rest() {
    let mut partial = jp_config::PartialAppConfig::empty();
    partial.style.code.color = Some(false);
    let original = ConfigDelta::from(partial);

    let event = InternalEvent::ConfigDelta(original);
    let mut json = serde_json::to_value(&event).unwrap();
    json["delta"]["style"]["code"]["removed_field"] = serde_json::json!("stale");

    let deserialized: InternalEvent = serde_json::from_value(json).unwrap();
    let InternalEvent::ConfigDelta(result) = deserialized else {
        panic!("expected ConfigDelta");
    };
    assert_eq!(result.delta.style.code.color, Some(false));
}

// --- deserialize_config_delta tests ---

#[test]
fn test_deserialize_config_delta_extracts_timestamp_and_delta() {
    let value = serde_json::json!({
        "timestamp": "2025-01-01 00:00:00.0",
        "delta": {
            "style": { "code": { "color": false } }
        }
    });

    let delta = deserialize_config_delta(&value);
    assert_eq!(delta.timestamp.to_string(), "2025-01-01 00:00:00 UTC");
    assert_eq!(delta.delta.style.code.color, Some(false));
}

#[test]
fn test_deserialize_config_delta_preserves_timestamp_on_bad_delta() {
    let value = serde_json::json!({
        "timestamp": "2024-12-25 18:30:00.0",
        "delta": "not an object at all"
    });

    let delta = deserialize_config_delta(&value);
    assert_eq!(delta.timestamp.to_string(), "2024-12-25 18:30:00 UTC");
    assert!(delta.delta.is_empty());
}

// --- from_parts / to_parts stream-level compat tests ---

#[test]
fn test_from_parts_tolerates_unknown_fields_in_config_deltas() {
    let mut partial = jp_config::PartialAppConfig::empty();
    partial.conversation.tools.defaults.run = Some(RunMode::Unattended);
    partial.style.code.color = Some(false);

    let mut stream = ConversationStream::new_test().with_config_delta(partial);
    stream.start_turn(ChatRequest::from("hello"));

    let (base_config, mut events) = stream.to_parts().unwrap();

    // Inject unknown fields into the config delta events in the stream.
    for event in &mut events {
        if event.get("type").and_then(|v| v.as_str()) == Some("config_delta")
            && let Some(delta) = event.get_mut("delta")
            && let Some(obj) = delta.as_object_mut()
        {
            obj.insert("removed_top_field".into(), serde_json::json!("stale"));
        }
    }

    let result = ConversationStream::from_parts(base_config, events)
        .unwrap()
        .with_created_at(stream.created_at);

    assert_eq!(result.len(), stream.len());

    let config = result.config().unwrap();
    assert_eq!(config.conversation.tools.defaults.run, RunMode::Unattended);
    assert!(!config.style.code.color);
}

#[test]
fn test_from_parts_tolerates_config_deltas_with_only_unknown_fields() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));

    let (base_config, mut events) = stream.to_parts().unwrap();

    // Inject config delta events where every field is unknown.
    events.push(serde_json::json!({
        "type": "config_delta",
        "timestamp": "2025-01-01 00:01:00.0",
        "delta": { "removed_section": { "a": 1 } }
    }));

    let result = ConversationStream::from_parts(base_config, events).unwrap();
    assert_eq!(result.len(), 2); // TurnStart + ChatRequest
}

// --- Compaction event invariant tests ---

fn make_compaction(from: usize, to: usize) -> Compaction {
    Compaction {
        timestamp: Utc.with_ymd_and_hms(2025, 7, 1, 12, 0, 0).unwrap(),
        from_turn: from,
        to_turn: to,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
    }
}

#[test]
fn test_compaction_not_counted_by_len() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    let len_before = stream.len();

    stream.add_compaction(make_compaction(0, 0));

    assert_eq!(stream.len(), len_before);
}

#[test]
fn test_compaction_not_counted_by_is_empty() {
    let mut stream = ConversationStream::new_test();
    assert!(stream.is_empty());

    stream.add_compaction(make_compaction(0, 0));

    assert!(
        stream.is_empty(),
        "Compaction alone should not make stream non-empty"
    );
}

#[test]
fn test_compaction_preserved_by_retain() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream.add_compaction(make_compaction(0, 0));

    // Retain nothing — all conversation events removed.
    stream.retain(|_| false);

    assert_eq!(stream.len(), 0);
    assert_eq!(
        stream.compactions().count(),
        1,
        "Compaction should survive retain"
    );
}

#[test]
fn test_compaction_skipped_by_iter() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream.add_compaction(make_compaction(0, 0));
    stream.push(ConversationEvent::new(
        ChatResponse::message("world"),
        Utc.with_ymd_and_hms(2025, 7, 1, 12, 0, 1).unwrap(),
    ));

    let events: Vec<_> = stream.iter().collect();
    // TurnStart + ChatRequest + ChatResponse = 3 events, no compaction.
    assert_eq!(events.len(), 3);
    assert!(
        events
            .iter()
            .all(|e| !matches!(&e.event.kind, EventKind::TurnStart(_)) || e.event.is_turn_start()),
        "Iterator should only yield ConversationEvents"
    );
}

#[test]
fn test_compaction_skipped_by_into_iter() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream.add_compaction(make_compaction(0, 0));
    stream.push(ConversationEvent::new(
        ChatResponse::message("world"),
        Utc.with_ymd_and_hms(2025, 7, 1, 12, 0, 1).unwrap(),
    ));

    assert_eq!(stream.into_iter().count(), 3);
}

#[test]
fn test_compaction_preserved_by_sanitize() {
    let mut stream = ConversationStream::new_test();
    stream.push(TurnStart);
    stream.push(ConversationEvent::new(
        ChatRequest::from("hello"),
        Utc.with_ymd_and_hms(2025, 7, 1, 12, 0, 0).unwrap(),
    ));
    stream.add_compaction(make_compaction(0, 0));
    stream.push(ConversationEvent::new(
        ChatResponse::message("hi"),
        Utc.with_ymd_and_hms(2025, 7, 1, 12, 0, 1).unwrap(),
    ));

    stream.sanitize();

    assert_eq!(
        stream.compactions().count(),
        1,
        "Compaction should survive sanitize"
    );
    assert_eq!(stream.len(), 3); // TurnStart + ChatRequest + ChatResponse
}

#[test]
fn test_compaction_roundtrip_via_to_parts_from_parts() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream.add_compaction(make_compaction(0, 0));

    let (base_config, events) = stream.to_parts().unwrap();

    // Verify the compaction event is present in serialized form.
    let compaction_count = events
        .iter()
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("compaction"))
        .count();
    assert_eq!(compaction_count, 1);

    // Roundtrip.
    let restored = ConversationStream::from_parts(base_config, events)
        .unwrap()
        .with_created_at(stream.created_at);

    assert_eq!(restored.len(), stream.len());
    assert_eq!(restored.compactions().count(), 1);

    let c = restored.compactions().next().unwrap();
    assert_eq!(c.from_turn, 0);
    assert_eq!(c.to_turn, 0);
    assert_eq!(c.reasoning, Some(ReasoningPolicy::Strip));
}

#[test]
fn test_compactions_accessor() {
    let mut stream = ConversationStream::new_test();
    assert_eq!(stream.compactions().count(), 0);

    stream.add_compaction(make_compaction(0, 2));
    stream.add_compaction(make_compaction(3, 5));

    let compactions: Vec<_> = stream.compactions().collect();
    assert_eq!(compactions.len(), 2);
    assert_eq!(compactions[0].from_turn, 0);
    assert_eq!(compactions[0].to_turn, 2);
    assert_eq!(compactions[1].from_turn, 3);
    assert_eq!(compactions[1].to_turn, 5);
}

#[test]
fn test_compaction_does_not_affect_config() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));

    let config_before = stream.config().unwrap().to_partial();
    stream.add_compaction(make_compaction(0, 0));
    let config_after = stream.config().unwrap().to_partial();

    assert_eq!(
        serde_json::to_value(&config_before).unwrap(),
        serde_json::to_value(&config_after).unwrap(),
    );
}

// --- turn_count, turn_at_time, resolve_compaction_range ---

#[test]
fn test_turn_count_empty() {
    let stream = ConversationStream::new_test();
    assert_eq!(stream.turn_count(), 0);
}

#[test]
fn test_turn_count_two_turns() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");
    stream.push(ConversationEvent::new(
        ChatResponse::message("hi"),
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.start_turn("bye");
    assert_eq!(stream.turn_count(), 2);
}

#[test]
fn test_turn_at_time() {
    let mut stream = ConversationStream::new_test();
    stream.push(ConversationEvent::new(
        TurnStart,
        Utc.with_ymd_and_hms(2025, 1, 1, 10, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatRequest::from("q1"),
        Utc.with_ymd_and_hms(2025, 1, 1, 10, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        TurnStart,
        Utc.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatRequest::from("q2"),
        Utc.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap(),
    ));

    let idx = |dt| stream.turn_at_time(dt).map(|t| t.index());

    // Before first turn.
    assert_eq!(
        idx(Utc.with_ymd_and_hms(2025, 1, 1, 9, 0, 0).unwrap()),
        None
    );
    // During first turn.
    assert_eq!(
        idx(Utc.with_ymd_and_hms(2025, 1, 1, 11, 0, 0).unwrap()),
        Some(0)
    );
    // Exactly at second turn start.
    assert_eq!(
        idx(Utc.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap()),
        Some(1)
    );
    // After second turn.
    assert_eq!(
        idx(Utc.with_ymd_and_hms(2025, 1, 1, 15, 0, 0).unwrap()),
        Some(1)
    );
}

#[test]
fn test_resolve_range_defaults() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("a");
    stream.start_turn("b");
    stream.start_turn("c");

    let range = resolve_range(&stream, None, None).unwrap();
    assert_eq!(range, CompactionRange {
        from_turn: 0,
        to_turn: 2
    });
}

#[test]
fn test_resolve_range_absolute() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("a");
    stream.start_turn("b");
    stream.start_turn("c");
    stream.start_turn("d");

    let range = resolve_range(
        &stream,
        Some(RangeBound::Absolute(1)),
        Some(RangeBound::Absolute(2)),
    )
    .unwrap();
    assert_eq!(range, CompactionRange {
        from_turn: 1,
        to_turn: 2
    });
}

#[test]
fn test_resolve_range_from_end() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("a");
    stream.start_turn("b");
    stream.start_turn("c");
    stream.start_turn("d"); // turns 0..3

    // FromEnd(1) on `to` means "1 before last" = turn 2.
    let range = resolve_range(&stream, None, Some(RangeBound::FromEnd(1))).unwrap();
    assert_eq!(range, CompactionRange {
        from_turn: 0,
        to_turn: 2
    });
}

#[test]
fn test_resolve_range_after_last_compaction() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("a");
    stream.start_turn("b");
    stream.start_turn("c");
    stream.start_turn("d");

    // No compactions yet → AfterLastCompaction resolves to 0.
    let range = resolve_range(&stream, Some(RangeBound::AfterLastCompaction), None).unwrap();
    assert_eq!(range.from_turn, 0);

    // Add a compaction covering turns 0..1.
    stream.add_compaction(make_compaction(0, 1));

    // AfterLastCompaction → to_turn + 1 = 2.
    let range = resolve_range(&stream, Some(RangeBound::AfterLastCompaction), None).unwrap();
    assert_eq!(range.from_turn, 2);
    assert_eq!(range.to_turn, 3);
}

#[test]
fn test_resolve_range_empty_stream() {
    let stream = ConversationStream::new_test();
    assert!(resolve_range(&stream, None, None).is_none());
}

#[test]
fn test_resolve_range_inverted_returns_none() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("a");
    stream.start_turn("b");

    // from=1, to=0 → empty range.
    let range = resolve_range(
        &stream,
        Some(RangeBound::Absolute(1)),
        Some(RangeBound::Absolute(0)),
    );
    assert!(range.is_none());
}

#[test]
fn test_resolve_range_clamps_beyond_max() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("a");
    stream.start_turn("b"); // turns 0..1

    let range = resolve_range(
        &stream,
        Some(RangeBound::Absolute(0)),
        Some(RangeBound::Absolute(99)),
    )
    .unwrap();
    assert_eq!(range.to_turn, 1);
}

/// Roundtrip a [`Compaction`] through [`InternalEvent`] serialization.
#[test]
fn test_internal_event_compaction_roundtrip() {
    let compaction = make_compaction(0, 5);
    let event = InternalEvent::Compaction(compaction.clone());
    let json = serde_json::to_value(&event).unwrap();

    assert_eq!(json["type"], "compaction");
    assert_eq!(json["from_turn"], 0);
    assert_eq!(json["to_turn"], 5);
    assert_eq!(json["reasoning"], "strip");

    let deserialized: InternalEvent = serde_json::from_value(json).unwrap();
    let InternalEvent::Compaction(result) = deserialized else {
        panic!("expected Compaction");
    };
    assert_eq!(result, compaction);
}
