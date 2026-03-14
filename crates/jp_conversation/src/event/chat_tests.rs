use serde_json::json;

use super::*;
use crate::{ConversationEvent, EventKind};

#[test]
fn chat_request_with_schema_roundtrip() {
    let request = ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(Map::from_iter([("type".into(), json!("object"))])),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["content"], "Extract contacts");
    assert_eq!(json["schema"]["type"], "object");

    let deserialized: ChatRequest = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, request);
}

#[test]
fn chat_request_without_schema_omits_field() {
    let request = ChatRequest::from("hello");
    let json = serde_json::to_value(&request).unwrap();
    assert!(json.get("schema").is_none());
}

#[test]
fn old_chat_request_json_deserializes_with_schema_none() {
    let json = json!({ "content": "hello" });
    let request: ChatRequest = serde_json::from_value(json).unwrap();
    assert_eq!(request.content, "hello");
    assert!(request.schema.is_none());
}

#[test]
fn structured_response_roundtrip() {
    let event = ConversationEvent::now(ChatResponse::structured(json!({"name": "Alice"})));
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["data"]["name"], "Alice");

    let deserialized: ConversationEvent = serde_json::from_value(json).unwrap();
    let resp = deserialized.as_chat_response().unwrap();
    assert!(resp.is_structured());
    assert_eq!(resp.as_structured_data(), Some(&json!({"name": "Alice"})));
}

#[test]
fn untagged_deserialization_distinguishes_variants() {
    let msg_json = json!({ "message": "hello" });
    let msg: ChatResponse = serde_json::from_value(msg_json).unwrap();
    assert!(msg.is_message());

    let reason_json = json!({ "reasoning": "let me think" });
    let reason: ChatResponse = serde_json::from_value(reason_json).unwrap();
    assert!(reason.is_reasoning());

    let structured_json = json!({ "data": { "key": "value" } });
    let structured: ChatResponse = serde_json::from_value(structured_json).unwrap();
    assert!(structured.is_structured());
}

#[test]
fn structured_within_event_kind_roundtrip() {
    let kind = EventKind::ChatResponse(ChatResponse::structured(json!([1, 2, 3])));
    let json = serde_json::to_value(&kind).unwrap();
    assert_eq!(json["type"], "chat_response");
    assert_eq!(json["data"], json!([1, 2, 3]));

    let deserialized: EventKind = serde_json::from_value(json).unwrap();
    match deserialized {
        EventKind::ChatResponse(ChatResponse::Structured { data }) => {
            assert_eq!(data, json!([1, 2, 3]));
        }
        other => panic!("expected Structured, got {other:?}"),
    }
}
