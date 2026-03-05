use chrono::TimeZone as _;
use serde_json::{Map, Value};

use super::*;
use crate::event::{InquiryQuestion, InquirySource};

#[test]
fn test_sanitize_orphaned_tool_calls_injects_directly_after_request() {
    let mut stream = ConversationStream::new_test();

    stream.add_chat_request("Hello");
    stream.add_tool_call_request(ToolCallRequest {
        id: "orphan_1".into(),
        name: "some_tool".into(),
        arguments: Map::new(),
    });
    stream.add_chat_response(ChatResponse::message("trailing"));

    stream.sanitize_orphaned_tool_calls();

    // Verify the response was injected.
    let response = stream.find_tool_call_response("orphan_1");
    assert!(response.is_some(), "Expected synthetic response for orphan");
    assert!(response.unwrap().result.is_err());
    assert!(response.unwrap().content().contains("interrupted"));

    // Verify ordering: request must be immediately followed by
    // its response — no events in between.
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

    stream.add_tool_call_request(ToolCallRequest {
        id: "matched_1".into(),
        name: "tool".into(),
        arguments: Map::new(),
    });
    stream.add_tool_call_response(ToolCallResponse {
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
    stream.add_tool_call_request(ToolCallRequest {
        id: "a".into(),
        name: "tool".into(),
        arguments: Map::new(),
    });
    stream.add_tool_call_request(ToolCallRequest {
        id: "b".into(),
        name: "tool".into(),
        arguments: Map::new(),
    });
    stream.add_tool_call_response(ToolCallResponse {
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
    stream.add_turn_start();
    assert_eq!(stream.len(), 1);

    stream.trim_trailing_empty_turn();
    assert_eq!(stream.len(), 0);
}

#[test]
fn test_trim_trailing_empty_turn_keeps_non_empty_turn() {
    let mut stream = ConversationStream::new_test();
    stream.add_turn_start();
    stream.add_chat_request("Hello");
    assert_eq!(stream.len(), 2);

    stream.trim_trailing_empty_turn();
    assert_eq!(stream.len(), 2, "Turn with events should not be removed");
}

#[test]
fn test_conversation_stream_serialization_roundtrip() {
    let mut stream = ConversationStream::new_test();

    insta::assert_json_snapshot!(&stream);

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

    insta::assert_json_snapshot!(&stream);
    let json = serde_json::to_string(&stream).unwrap();
    let stream2 = serde_json::from_str::<ConversationStream>(&json).unwrap();
    assert_eq!(stream, stream2);
}

#[test]
fn test_serialization_base64_encodes_tool_call_fields() {
    let mut stream = ConversationStream::new_test();

    let mut args = Map::new();
    args.insert("path".into(), Value::String("src/main.rs".into()));

    stream.add_tool_call_request(ToolCallRequest {
        id: "tc1".into(),
        name: "read_file".into(),
        arguments: args,
    });
    stream.add_tool_call_response(ToolCallResponse {
        id: "tc1".into(),
        result: Ok("file contents here".into()),
    });

    // Serialize to JSON (as storage would).
    let json = serde_json::to_string(&stream).unwrap();

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

    // Roundtrip should recover the original values.
    let stream2 = serde_json::from_str::<ConversationStream>(&json).unwrap();

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

    // --from removed the ToolCallRequest but kept the response.
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
            InquirySource::Tool {
                name: "read_file".into(),
            },
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
    stream.add_turn_start();
    stream.add_turn_start();
    stream.add_turn_start();
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
    stream.add_turn_start();
    stream.push(ConversationEvent::new(
        ChatRequest::from("first"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("reply"),
        Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 1).unwrap(),
    ));
    stream.add_turn_start();
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
fn test_sanitize_noop_on_healthy_stream() {
    let mut stream = ConversationStream::new_test();

    stream.add_turn_start();
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
