use serde_json::Map;

use crate::{
    ConversationStream, StreamError,
    event::{
        ChatRequest, ChatResponse, InquiryQuestion, InquiryRequest, InquiryResponse, InquirySource,
        ToolCallRequest, ToolCallResponse, TurnStart,
    },
};

#[test]
fn build_flushes_buffered_events() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("world"))
        .build()
        .unwrap();

    // TurnStart + ChatRequest + ChatResponse
    assert_eq!(stream.len(), 3);
    assert!(stream.last().unwrap().event.is_chat_response());
}

#[test]
fn build_flushes_multiple_events() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("thinking..."))
        .add_tool_call_request(ToolCallRequest {
            id: "tc1".into(),
            name: "read_file".into(),
            arguments: Map::new(),
        })
        .build()
        .unwrap();

    // TurnStart + ChatRequest + ChatResponse + ToolCallRequest
    assert_eq!(stream.len(), 4);
}

#[test]
fn with_methods_chain_on_binding() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    // with_* returns &mut Self, so we use a binding and call build at the end.
    let mut turn = stream.current_turn_mut();
    turn.with_chat_response(ChatResponse::message("hi"));
    turn.with_tool_call_request(ToolCallRequest {
        id: "tc1".into(),
        name: "tool".into(),
        arguments: Map::new(),
    });
    turn.with_tool_call_response(ToolCallResponse {
        id: "tc1".into(),
        result: Ok("done".into()),
    });
    turn.build().unwrap();

    // TurnStart + ChatRequest + ChatResponse + ToolCallRequest + ToolCallResponse
    assert_eq!(stream.len(), 5);
}

#[test]
fn add_event_rejects_turn_start() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    stream
        .current_turn_mut()
        .add_event(TurnStart)
        .build()
        .unwrap();

    // TurnStart + ChatRequest only — the TurnStart from add_event was dropped
    assert_eq!(stream.len(), 2);
}

#[test]
fn with_event_rejects_turn_start() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    let mut turn = stream.current_turn_mut();
    turn.with_event(TurnStart);
    turn.build().unwrap();

    // TurnStart + ChatRequest only
    assert_eq!(stream.len(), 2);
}

#[test]
fn current_turn_mut_injects_turn_start_when_empty() {
    let mut stream = ConversationStream::new_test();
    // No start_turn called — current_turn_mut should inject one.
    stream
        .current_turn_mut()
        .add_chat_request(ChatRequest::from("hello"))
        .build()
        .unwrap();

    // Injected TurnStart + ChatRequest
    assert_eq!(stream.len(), 2);
    assert!(stream.first().unwrap().event.is_turn_start());
}

#[test]
fn empty_build_is_noop() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");
    let len_before = stream.len();

    stream.current_turn_mut().build().unwrap();

    assert_eq!(stream.len(), len_before);
}

// -- Validation tests --

#[test]
fn tool_call_response_requires_matching_request() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    let result = stream
        .current_turn_mut()
        .add_tool_call_response(ToolCallResponse {
            id: "nonexistent".into(),
            result: Ok("data".into()),
        })
        .build();

    assert!(matches!(
        result,
        Err(StreamError::OrphanedToolCallResponse { ref id }) if id == "nonexistent"
    ));
    // Stream unchanged — the response was not flushed.
    assert_eq!(stream.len(), 2); // TurnStart + ChatRequest only
}

#[test]
fn tool_call_response_accepted_when_request_exists() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");
    stream
        .current_turn_mut()
        .add_tool_call_request(ToolCallRequest {
            id: "tc1".into(),
            name: "read_file".into(),
            arguments: Map::new(),
        })
        .build()
        .unwrap();

    stream
        .current_turn_mut()
        .add_tool_call_response(ToolCallResponse {
            id: "tc1".into(),
            result: Ok("contents".into()),
        })
        .build()
        .unwrap();

    // TurnStart + ChatRequest + ToolCallRequest + ToolCallResponse
    assert_eq!(stream.len(), 4);
}

#[test]
fn tool_call_request_and_response_in_same_buffer() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    // Both request and response in the same build() call.
    stream
        .current_turn_mut()
        .add_tool_call_request(ToolCallRequest {
            id: "tc1".into(),
            name: "tool".into(),
            arguments: Map::new(),
        })
        .add_tool_call_response(ToolCallResponse {
            id: "tc1".into(),
            result: Ok("ok".into()),
        })
        .build()
        .unwrap();

    assert_eq!(stream.len(), 4);
}

#[test]
fn duplicate_tool_call_response_rejected() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");
    stream
        .current_turn_mut()
        .add_tool_call_request(ToolCallRequest {
            id: "tc1".into(),
            name: "tool".into(),
            arguments: Map::new(),
        })
        .add_tool_call_response(ToolCallResponse {
            id: "tc1".into(),
            result: Ok("first".into()),
        })
        .build()
        .unwrap();

    let result = stream
        .current_turn_mut()
        .add_tool_call_response(ToolCallResponse {
            id: "tc1".into(),
            result: Ok("second".into()),
        })
        .build();

    assert!(matches!(
        result,
        Err(StreamError::DuplicateToolCallResponse { ref id }) if id == "tc1"
    ));
}

#[test]
fn inquiry_response_requires_matching_request() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");

    let result = stream
        .current_turn_mut()
        .add_inquiry_response(InquiryResponse::boolean("nonexistent", true))
        .build();

    assert!(matches!(
        result,
        Err(StreamError::OrphanedInquiryResponse { ref id }) if id == "nonexistent"
    ));
    assert_eq!(stream.len(), 2);
}

#[test]
fn inquiry_response_accepted_when_request_exists() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");
    stream
        .current_turn_mut()
        .add_inquiry_request(InquiryRequest::new(
            "iq1",
            InquirySource::Assistant,
            InquiryQuestion::boolean("proceed?".into()),
        ))
        .build()
        .unwrap();

    stream
        .current_turn_mut()
        .add_inquiry_response(InquiryResponse::boolean("iq1", true))
        .build()
        .unwrap();

    assert_eq!(stream.len(), 4);
}

#[test]
fn duplicate_inquiry_response_rejected() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("hello");
    stream
        .current_turn_mut()
        .add_inquiry_request(InquiryRequest::new(
            "iq1",
            InquirySource::Assistant,
            InquiryQuestion::boolean("proceed?".into()),
        ))
        .add_inquiry_response(InquiryResponse::boolean("iq1", true))
        .build()
        .unwrap();

    let result = stream
        .current_turn_mut()
        .add_inquiry_response(InquiryResponse::boolean("iq1", false))
        .build();

    assert!(matches!(
        result,
        Err(StreamError::DuplicateInquiryResponse { ref id }) if id == "iq1"
    ));
}
