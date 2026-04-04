use jp_config::AppConfig;
use jp_conversation::event::ChatResponse;
use jp_llm::event::FinishReason;
use jp_printer::{OutputFormat, Printer};
use serde_json::{Map, json};

use super::{super::state::TurnState, *};

#[test]
fn test_transitions_to_executing_on_tool_call() {
    let mut _turn_state = TurnState::default();
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));
    assert_eq!(coordinator.current_phase(), TurnPhase::Streaming);

    // Simulate tool call part
    let tool_call = ToolCallRequest {
        id: "call_1".into(),
        name: "tool".into(),
        arguments: serde_json::Map::new(),
    };

    coordinator.handle_event(
        &mut stream,
        Event::tool_call_start(0, tool_call.id.clone(), tool_call.name.clone()),
    );

    // Simulate flush
    coordinator.handle_event(&mut stream, Event::flush(0));

    // Simulate finished
    let action = coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    assert_eq!(coordinator.current_phase(), TurnPhase::Executing);
    match action {
        Action::ExecuteTools(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, "call_1");
        }
        _ => panic!("Expected ExecuteTools action"),
    }
}

#[test]
fn test_transitions_to_complete_no_tools() {
    let mut _turn_state = TurnState::default();
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Simulate message
    coordinator.handle_event(&mut stream, Event::message(0, "Hi"));

    let action = coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    assert_eq!(coordinator.current_phase(), TurnPhase::Complete);
    match action {
        Action::Done => {}
        _ => panic!("Expected Done action"),
    }
}

#[test]
fn test_continues_after_tool_execution() {
    let mut _turn_state = TurnState::default();
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style);

    // Drive the coordinator to Executing by simulating a tool call in the
    // stream, then test that handle_tool_responses transitions back to
    // Streaming.
    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Simulate LLM producing a tool call (Part + Flush + Finished).
    coordinator.handle_event(&mut stream, Event::tool_call_start(0, "1", "t"));
    coordinator.handle_event(&mut stream, Event::flush(0));
    coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    assert_eq!(coordinator.current_phase(), TurnPhase::Executing);

    // Handle responses
    let responses = vec![ToolCallResponse {
        id: "1".into(),
        result: Ok("output".into()),
    }];

    let action = coordinator.handle_tool_responses(&mut stream, responses);

    assert_eq!(coordinator.current_phase(), TurnPhase::Streaming);
    match action {
        Action::SendFollowUp => {}
        _ => panic!("Expected SendFollowUp action"),
    }

    // Stream has: TurnStart + ChatRequest + ToolCallRequest + ToolCallResponse
    assert_eq!(stream.len(), 4);
}

#[test]
fn test_peek_partial_content() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Initially no partial content
    assert_eq!(coordinator.peek_partial_content(), None);

    // Add a partial message (not flushed)
    coordinator.handle_event(&mut stream, Event::message(0, "Hello "));
    coordinator.handle_event(&mut stream, Event::message(0, "world"));

    // Should have partial content
    assert_eq!(
        coordinator.peek_partial_content(),
        Some("Hello world".to_string())
    );

    // Flush clears the buffer
    coordinator.handle_event(&mut stream, Event::flush(0));
    assert_eq!(coordinator.peek_partial_content(), None);
}

#[test]
fn test_buffered_markdown_flushed_before_tool_call() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(Arc::clone(&printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // LLM sends a partial markdown line (no trailing newline / block boundary)
    coordinator.handle_event(
        &mut stream,
        Event::message(0, "Now wire the config into `ChatResponseRenderer`:"),
    );

    // Nothing rendered yet — buffer is waiting for a complete block
    printer.flush();
    assert_eq!(*out.lock(), "");

    // LLM immediately follows with a tool call (no newline in between)
    let tool_call = ToolCallRequest {
        id: "call_1".into(),
        name: "fs_read_file".into(),
        arguments: serde_json::Map::new(),
    };
    coordinator.handle_event(
        &mut stream,
        Event::tool_call_start(1, tool_call.id.clone(), tool_call.name.clone()),
    );

    // The buffered markdown should now be flushed
    printer.flush();
    assert!(
        out.lock().contains("Now wire the config"),
        "Expected buffered markdown to be flushed before tool call, got: {:?}",
        *out.lock()
    );
}

#[test]
fn test_prepare_continuation() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Add partial content
    coordinator.handle_event(&mut stream, Event::message(0, "Partial"));
    assert!(coordinator.peek_partial_content().is_some());

    // Prepare continuation resets state
    coordinator.prepare_continuation();

    assert_eq!(coordinator.current_phase(), TurnPhase::Streaming);
    assert_eq!(coordinator.peek_partial_content(), None);
}

/// Tests the multi-part tool call flow: an initial Part with name+id
/// (empty arguments) marks the tool as "preparing", and the Flush
/// after the final Part moves it to `pending_tool_calls`.
#[test]
fn test_multi_part_tool_call_deferred_to_flush() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // First Part: name + id (from content_block_start)
    coordinator.handle_event(
        &mut stream,
        Event::tool_call_start(1, "call_99", "fs_create_file"),
    );

    // Not in pending_tool_calls yet (no flush)
    assert!(coordinator.pending_tool_calls.is_empty());

    // Argument chunks arrive incrementally
    coordinator.handle_event(
        &mut stream,
        Event::tool_call_args(1, r#"{"path": "test.rs"}"#),
    );

    // Still not in pending (no flush yet)
    assert!(coordinator.pending_tool_calls.is_empty());

    // Flush finalizes the tool call
    coordinator.handle_event(&mut stream, Event::flush(1));
    assert_eq!(coordinator.pending_tool_calls.len(), 1);
    assert_eq!(coordinator.pending_tool_calls[0].id, "call_99");
    assert_eq!(coordinator.pending_tool_calls[0].name, "fs_create_file");
    assert_eq!(
        coordinator.pending_tool_calls[0].arguments["path"],
        "test.rs"
    );

    // Finish should transition to Executing
    let action = coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    assert_eq!(coordinator.current_phase(), TurnPhase::Executing);
    match action {
        Action::ExecuteTools(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "fs_create_file");
        }
        _ => panic!("Expected ExecuteTools action"),
    }
}

#[test]
fn test_structured_output_rendered_as_json_code_fence() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(Arc::clone(&printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(Map::from_iter([("type".into(), json!("object"))])),
    });

    // Streamed structured chunks
    coordinator.handle_event(&mut stream, Event::structured(0, "{\"name\""));
    coordinator.handle_event(&mut stream, Event::structured(0, ": \"Alice\"}"));

    coordinator.handle_event(&mut stream, Event::flush(0));
    let action = coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    printer.flush();
    let output = out.lock().clone();

    // Should render as a fenced JSON code block
    assert!(
        output.contains("```json\n"),
        "Expected opening code fence, got: {output:?}"
    );
    assert!(
        output.contains("{\"name\": \"Alice\"}"),
        "Expected JSON content, got: {output:?}"
    );
    assert!(
        output.contains("\n```\n"),
        "Expected closing code fence, got: {output:?}"
    );

    // Turn should complete
    assert_eq!(coordinator.current_phase(), TurnPhase::Complete);
    assert!(matches!(action, Action::Done));
}

#[test]
fn test_structured_output_persisted_with_parsed_json() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(Map::from_iter([("type".into(), json!("object"))])),
    });

    coordinator.handle_event(&mut stream, Event::structured(0, "{\"name\": \"Alice\"}"));
    coordinator.handle_event(&mut stream, Event::flush(0));
    coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    // The flushed event should contain parsed JSON, not a string.
    let data = stream
        .iter()
        .rev()
        .find_map(|e| {
            e.as_chat_response()
                .and_then(ChatResponse::as_structured_data)
                .cloned()
        })
        .unwrap();

    assert_eq!(data, json!({"name": "Alice"}));
}

#[test]
fn test_structured_not_routed_to_chat_renderer() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(Arc::clone(&printer), AppConfig::new_test().style);

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Send a structured chunk — it must NOT go through the markdown formatter.
    coordinator.handle_event(&mut stream, Event::structured(0, "# heading"));
    coordinator.handle_event(&mut stream, Event::flush(0));
    coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    printer.flush();
    let output = out.lock().clone();

    // The chat renderer would have formatted "# heading" as a styled header.
    // The structured renderer outputs it raw inside a code fence.
    assert!(
        output.contains("# heading"),
        "Expected raw text (not markdown-formatted), got: {output:?}"
    );
    assert!(
        output.contains("```json"),
        "Expected code fence, got: {output:?}"
    );
}
