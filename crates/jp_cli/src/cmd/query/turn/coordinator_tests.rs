use jp_config::AppConfig;
use jp_conversation::event::{ChatResponse, ToolCallRequest};
use jp_llm::event::FinishReason;
use jp_printer::{OutputFormat, Printer};
use serde_json::{Map, json};

use super::{super::state::TurnState, *};
use crate::cmd::query::interrupt::InterruptAction;

/// Strip ANSI escape codes for readable assertions on captured stdout.
fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8(bytes).expect("valid utf-8 after stripping ANSI")
}

#[test]
fn test_transitions_to_executing_on_tool_call() {
    let mut _turn_state = TurnState::default();
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

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
    assert!(
        matches!(action.action, Action::ExecuteTools),
        "expected ExecuteTools, got {action:?}"
    );
    // The actual list of tool calls to execute is derived from the stream
    // (the durable source of truth), not carried in the Action.
    let tool_calls: Vec<_> = stream
        .iter()
        .filter_map(|e| e.event.as_tool_call_request())
        .collect();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_1");
}

#[test]
fn finish_notice_silent_for_completed_and_retry() {
    assert!(finish_notice(&FinishReason::Completed, &[]).is_none());
    assert!(finish_notice(&FinishReason::Retry, &[]).is_none());
}

#[test]
fn finish_notice_max_tokens_plain() {
    let msg = finish_notice(&FinishReason::MaxTokens, &[]).expect("notice");
    assert!(msg.contains("max output tokens"));
    assert!(!msg.contains("tool call"));
}

#[test]
fn finish_notice_max_tokens_names_dropped_tool() {
    let dropped = vec!["fs_modify_file".to_string()];
    let msg = finish_notice(&FinishReason::MaxTokens, &dropped).expect("notice");
    assert!(msg.contains("fs_modify_file"));
    assert!(msg.contains("tool call"));
}

#[test]
fn finish_notice_other_uses_unquoted_string_detail() {
    let reason = FinishReason::Other(json!("content_filter"));
    let msg = finish_notice(&reason, &[]).expect("notice");
    assert!(msg.contains("content_filter"));
    assert!(
        !msg.contains('"'),
        "string detail should be unquoted: {msg}"
    );
}

#[test]
fn finish_notice_refused_renders_category_and_explanation() {
    let reason = FinishReason::Refused {
        category: Some("cyber".to_string()),
        explanation: Some("declined for safety".to_string()),
    };
    let msg = finish_notice(&reason, &[]).expect("notice");
    assert_eq!(
        msg,
        "The model declined this request (cyber): declined for safety"
    );
}

#[test]
fn finish_notice_refused_uncategorized() {
    let reason = FinishReason::Refused {
        category: None,
        explanation: None,
    };
    let msg = finish_notice(&reason, &[]).expect("notice");
    assert_eq!(msg, "The model declined this request.");
}

/// A refusal after buffered (unflushed) partial text discards it: the
/// `FinishReason::Refused` contract says partial output must not persist.
#[test]
fn test_refused_discards_buffered_partial_text() {
    let mut stream = ConversationStream::new_test();
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("explain X"));
    // Partial assistant text streams but never flushes before the refusal.
    coordinator.handle_event(&mut stream, Event::message(0, "Here is the par"));
    coordinator.handle_event(
        &mut stream,
        Event::Finished(FinishReason::Refused {
            category: Some("cyber".to_string()),
            explanation: Some("declined".to_string()),
        }),
    );

    assert!(
        !stream.iter().any(|e| e.event.is_chat_response()),
        "refusal must discard partial assistant output"
    );
    // The user's request is retained.
    assert!(stream.iter().any(|e| e.event.is_chat_request()));
}

/// A refusal after *flushed* partial text also discards it: the block was
/// already pushed into the stream during streaming, so the refused path must
/// remove it, not just drop the buffer.
#[test]
fn test_refused_discards_flushed_partial_text() {
    let mut stream = ConversationStream::new_test();
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("explain X"));
    coordinator.handle_event(&mut stream, Event::message(0, "A complete sentence."));
    coordinator.handle_event(&mut stream, Event::flush(0));
    // Sanity: the flushed block reached the stream before the refusal.
    assert!(stream.iter().any(|e| e.event.is_chat_response()));

    coordinator.handle_event(
        &mut stream,
        Event::Finished(FinishReason::Refused {
            category: None,
            explanation: None,
        }),
    );

    assert!(
        !stream.iter().any(|e| e.event.is_chat_response()),
        "refusal must discard already-flushed partial assistant output"
    );
    assert!(stream.iter().any(|e| e.event.is_chat_request()));
}

/// A max-tokens finish surfaces a chrome notice on stderr, never on stdout.
#[test]
fn test_max_tokens_finish_emits_chrome_notice() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, err) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(
        printer.clone(),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));
    coordinator.handle_event(&mut stream, Event::Finished(FinishReason::MaxTokens));
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.contains("max output tokens"),
        "truncation notice should be on stderr.\nstderr:\n{chrome}"
    );
    drop(chrome);

    let stdout = out.lock();
    assert!(
        !stdout.contains("max output tokens"),
        "notice must not leak onto stdout.\nstdout:\n{stdout}"
    );
}

/// In JSON mode the notice is suppressed: it must not reach stderr, and stdout
/// carries only the NDJSON conversation events.
/// This guards the printer/sink split in `TurnCoordinator::new`.
#[test]
fn test_max_tokens_finish_suppressed_in_json_mode() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, err) = Printer::memory(OutputFormat::Json);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(
        printer.clone(),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));
    coordinator.handle_event(&mut stream, Event::Finished(FinishReason::MaxTokens));
    printer.flush();

    // The notice is chrome; in JSON mode it goes to the sink, so stderr is empty.
    let chrome = err.lock();
    assert!(
        chrome.is_empty(),
        "JSON mode must not emit chrome to stderr.\nstderr:\n{chrome}"
    );
    drop(chrome);

    // stdout carries the NDJSON conversation events and never the notice.
    let stdout = out.lock();
    assert!(
        !stdout.contains("max output tokens"),
        "notice must never reach stdout.\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("test"),
        "stdout should contain the NDJSON conversation events.\nstdout:\n{stdout}"
    );
}

#[test]
fn test_transitions_to_complete_no_tools() {
    let mut _turn_state = TurnState::default();
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Simulate message
    coordinator.handle_event(&mut stream, Event::message(0, "Hi"));

    let action = coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    assert_eq!(coordinator.current_phase(), TurnPhase::Complete);
    match action.action {
        Action::Done => {}
        _ => panic!("Expected Done action"),
    }
}

#[test]
fn test_continues_after_tool_execution() {
    let mut _turn_state = TurnState::default();
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

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
fn test_peek_partial_events() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Initially no partial content
    assert!(coordinator.peek_partial_events().is_empty());

    // Add a partial message (not flushed)
    coordinator.handle_event(&mut stream, Event::message(0, "Hello "));
    coordinator.handle_event(&mut stream, Event::message(0, "world"));

    // Should have partial content as a Message response
    let partial = coordinator.peek_partial_events();
    assert_eq!(partial.len(), 1);
    assert!(
        matches!(&partial[0], ChatResponse::Message { message } if message == "Hello world"),
        "got {partial:?}"
    );

    // Flush clears the buffer
    coordinator.handle_event(&mut stream, Event::flush(0));
    assert!(coordinator.peek_partial_events().is_empty());
}

#[test]
fn test_complete_early_commits_partials() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Unflushed partial content sits in the event builder.
    coordinator.handle_event(&mut stream, Event::message(0, "partial answer"));
    let len_before = stream.len();

    coordinator.complete_early(&mut stream);

    assert_eq!(coordinator.current_phase(), TurnPhase::Complete);
    // The partial message was committed to the stream.
    assert_eq!(stream.len(), len_before + 1);
}

#[test]
fn test_complete_early_without_partials() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));
    let len_before = stream.len();

    coordinator.complete_early(&mut stream);

    assert_eq!(coordinator.current_phase(), TurnPhase::Complete);
    assert_eq!(stream.len(), len_before);
}

#[test]
fn test_buffered_markdown_flushed_before_tool_call() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(
        Arc::clone(&printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // LLM sends a partial markdown line (no trailing newline / block boundary)
    coordinator.handle_event(
        &mut stream,
        Event::message(0, "Now wire the config into `ChatResponseRenderer`:"),
    );

    // The assistant role header is emitted on the first chunk, but the
    // partial markdown content itself is still in the buffer waiting for
    // a complete block.
    printer.flush();
    assert!(
        !out.lock().contains("Now wire the config"),
        "Partial markdown content should still be buffered, got: {:?}",
        *out.lock()
    );

    // The turn loop resolves the tool-call boundary before dispatching a tool
    // start; the boundary drains the buffered markdown so it lands before the
    // tool header.
    coordinator.enter_tool_call(true);

    // The buffered markdown should now be flushed
    printer.flush();
    assert!(
        out.lock().contains("Now wire the config"),
        "Expected buffered markdown to be flushed at the tool-call boundary, got: {:?}",
        *out.lock()
    );
}

#[test]
fn test_prepare_continuation() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Add partial content
    coordinator.handle_event(&mut stream, Event::message(0, "Partial"));
    assert!(!coordinator.peek_partial_events().is_empty());

    // Prepare continuation resets state
    coordinator.prepare_continuation();

    assert_eq!(coordinator.current_phase(), TurnPhase::Streaming);
    assert!(coordinator.peek_partial_events().is_empty());
}

/// Tests the multi-part tool call flow: an initial Part with name+id (empty
/// arguments) marks the tool as "preparing", and the Flush after the final Part
/// appends the complete request to the conversation stream.
/// The state machine transitions to Executing on Finished if any unresponded
/// tool-call request is in the current turn.
#[test]
fn test_multi_part_tool_call_deferred_to_flush() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    let tool_call_count = |stream: &ConversationStream| {
        stream
            .iter()
            .filter(|e| e.event.as_tool_call_request().is_some())
            .count()
    };

    // First Part: name + id (from content_block_start)
    coordinator.handle_event(
        &mut stream,
        Event::tool_call_start(1, "call_99", "fs_create_file"),
    );

    // Not appended to the stream yet (no flush)
    assert_eq!(tool_call_count(&stream), 0);

    // Argument chunks arrive incrementally
    coordinator.handle_event(
        &mut stream,
        Event::tool_call_args(1, r#"{"path": "test.rs"}"#),
    );

    // Still not in the stream (no flush yet)
    assert_eq!(tool_call_count(&stream), 0);

    // Flush finalizes the tool call and appends it to the stream
    coordinator.handle_event(&mut stream, Event::flush(1));
    let requests: Vec<_> = stream
        .iter()
        .filter_map(|e| e.event.as_tool_call_request())
        .cloned()
        .collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].id, "call_99");
    assert_eq!(requests[0].name, "fs_create_file");
    assert_eq!(requests[0].arguments["path"], "test.rs");

    // Finish should transition to Executing because there's an unresponded
    // tool-call request in the current turn.
    let action = coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    assert_eq!(coordinator.current_phase(), TurnPhase::Executing);
    assert!(
        matches!(action.action, Action::ExecuteTools),
        "expected ExecuteTools, got {action:?}"
    );
}

#[test]
fn test_structured_output_rendered_as_json_code_fence() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(
        Arc::clone(&printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(Map::from_iter([("type".into(), json!("object"))])),
        author: None,
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
    assert!(matches!(action.action, Action::Done));
}

#[test]
fn test_structured_output_persisted_with_parsed_json() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(Map::from_iter([("type".into(), json!("object"))])),
        author: None,
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
    let mut coordinator = TurnCoordinator::new(
        Arc::clone(&printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

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

/// Regression: if the user interrupts before any chunk has arrived and chooses
/// Continue, the next assistant event MUST emit a fresh role header.
/// The previous behaviour forced `assistant_header_rendered = true` in
/// `reset_for_continuation`, which suppressed the header unconditionally and
/// left the resumed output without a `── jp …` boundary.
#[test]
fn interrupt_continue_before_first_chunk_emits_assistant_header_on_resume() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(
        Arc::clone(&printer),
        AppConfig::new_test().style,
        None,
        None,
        Some("anthropic/test".into()),
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("hello"));

    // No chunks have arrived yet — the assistant header has NOT been emitted.
    coordinator.handle_streaming_interrupt(InterruptAction::Continue, &mut stream);

    // Resumed cycle delivers its first chunk.
    coordinator.handle_event(&mut stream, Event::message(0, "hi there"));
    coordinator.handle_event(&mut stream, Event::flush(0));
    coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    printer.flush();
    let output = strip_ansi(&out.lock());
    assert!(
        output.contains("\u{2500}\u{2500} jp "),
        "resumed assistant content must be preceded by a `── jp` header, got: {output:?}"
    );
}

/// Regression: an editor-composed Reply interrupt inserts a new `ChatRequest`
/// boundary whose text never appeared on the terminal, so live mode must echo
/// it: a labeled user header AND a fresh assistant header for the following
/// content, matching what replay renders for this `ChatRequest`.
#[test]
fn interrupt_reply_from_editor_renders_user_header_for_new_request() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(
        Arc::clone(&printer),
        AppConfig::new_test().style,
        Some("alice".into()),
        None,
        Some("anthropic/test".into()),
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("first question"));

    // Some assistant content arrives so the assistant header is emitted.
    coordinator.handle_event(&mut stream, Event::message(0, "partial answer"));

    // User interrupts with a follow-up reply composed in the external editor.
    coordinator.handle_streaming_interrupt(
        InterruptAction::Reply {
            content: "actually, ignore that".into(),
            from_editor: true,
        },
        &mut stream,
    );

    // Resumed cycle delivers the new assistant content.
    coordinator.handle_event(&mut stream, Event::message(1, "new answer"));
    coordinator.handle_event(&mut stream, Event::flush(1));
    coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    printer.flush();
    let output = strip_ansi(&out.lock());

    // The labeled user header for the Reply must show up between the
    // partial answer and the resumed assistant content.
    let alice_idx = output
        .find("\u{2500}\u{2500} alice ")
        .unwrap_or_else(|| panic!("expected a `── alice` header for the Reply, got: {output:?}"));

    // And a fresh assistant header must follow the user boundary.
    let after_alice = &output[alice_idx..];
    assert!(
        after_alice.contains("\u{2500}\u{2500} jp "),
        "expected a fresh `── jp` header after the Reply, got: {output:?}"
    );
}

/// An inline-composed Reply is already visible in scrollback on the widget's
/// own line, so live mode must NOT echo a labeled user header for it — but the
/// next assistant chunk must still open with a fresh assistant header.
#[test]
fn interrupt_reply_inline_skips_user_header_but_resets_assistant_header() {
    let mut stream = ConversationStream::new_test();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let printer = Arc::new(printer);
    let mut coordinator = TurnCoordinator::new(
        Arc::clone(&printer),
        AppConfig::new_test().style,
        Some("alice".into()),
        None,
        Some("anthropic/test".into()),
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("first question"));

    // Some assistant content arrives so the assistant header is emitted.
    coordinator.handle_event(&mut stream, Event::message(0, "partial answer"));

    // User interrupts with a follow-up reply submitted from the inline widget.
    coordinator.handle_streaming_interrupt(
        InterruptAction::Reply {
            content: "actually, ignore that".into(),
            from_editor: false,
        },
        &mut stream,
    );

    // Resumed cycle delivers the new assistant content.
    coordinator.handle_event(&mut stream, Event::message(1, "new answer"));
    coordinator.handle_event(&mut stream, Event::flush(1));
    coordinator.handle_event(&mut stream, Event::Finished(FinishReason::Completed));

    printer.flush();
    let output = strip_ansi(&out.lock());

    // No echoed user header: the widget's own line is the live rendering.
    assert!(
        !output.contains("\u{2500}\u{2500} alice "),
        "expected no `── alice` header for an inline reply, got: {output:?}"
    );

    // The reply still lands in the stream as a `ChatRequest`.
    assert!(
        stream
            .iter()
            .filter_map(|e| e.event.as_chat_request())
            .any(|r| r.content == "actually, ignore that"),
        "expected the inline reply recorded as a ChatRequest"
    );

    // And the resumed assistant content opens with a fresh `── jp` header:
    // one for the partial answer, a second after the inline reply.
    assert_eq!(
        output.matches("\u{2500}\u{2500} jp ").count(),
        2,
        "expected two `── jp` headers (partial answer + resumed content), got: {output:?}"
    );
}

/// Regression: interrupting mid-reasoning and replying must preserve the
/// partial reasoning as a `ChatResponse::Reasoning` event, committed before the
/// user's reply.
/// Dropping it made the model re-reason from scratch on resume, leaving the
/// user's interjection orphaned against reasoning the model no longer has.
#[test]
fn interrupt_reply_during_reasoning_preserves_partial_reasoning() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        Some("alice".into()),
        None,
        Some("anthropic/test".into()),
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("first question"));

    // The assistant is mid-reasoning when the user interrupts: reasoning
    // chunks have arrived, but no Flush and no message text.
    coordinator.handle_event(&mut stream, Event::reasoning(0, "I should consider "));
    coordinator.handle_event(&mut stream, Event::reasoning(0, "the trade-offs"));

    coordinator.handle_streaming_interrupt(
        InterruptAction::Reply {
            content: "actually, do X instead".into(),
            from_editor: false,
        },
        &mut stream,
    );

    // The partial reasoning is committed as a Reasoning event...
    let responses: Vec<_> = stream
        .iter()
        .filter_map(|e| e.event.as_chat_response().cloned())
        .collect();
    assert_eq!(
        responses.len(),
        1,
        "expected one reasoning response, got {responses:?}"
    );
    assert!(
        matches!(
            &responses[0],
            ChatResponse::Reasoning { reasoning } if reasoning == "I should consider the trade-offs"
        ),
        "expected partial reasoning preserved, got {responses:?}"
    );

    // ...and the reply lands after it as a new user request.
    let requests: Vec<_> = stream
        .iter()
        .filter_map(|e| e.event.as_chat_request())
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].content, "actually, do X instead");

    // The reasoning must precede the reply in the stream.
    let order: Vec<_> = stream
        .iter()
        .filter_map(|e| {
            if e.event.as_chat_response().is_some() {
                Some("response")
            } else if e.event.as_chat_request().is_some() {
                Some("request")
            } else {
                None
            }
        })
        .collect();
    assert_eq!(order, ["request", "response", "request"]);
}

/// A `Flush` that produces a `ToolCallRequest` surfaces it as a committed
/// event.
/// The shell uses this signal directly instead of inspecting the stream tail,
/// so the dispatch trigger is unambiguous.
#[test]
fn flush_producing_tool_call_surfaces_request_as_committed_event() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    coordinator.handle_event(
        &mut stream,
        Event::tool_call_start(1, "call_42", "fs_read_file"),
    );
    coordinator.handle_event(
        &mut stream,
        Event::tool_call_args(1, r#"{"path":"foo.rs"}"#),
    );

    let action = coordinator.handle_event(&mut stream, Event::flush(1));

    assert!(matches!(action.action, Action::Continue));
    match action.committed {
        CommittedEvent::ToolCallRequest(req) => {
            assert_eq!(req.id, "call_42");
            assert_eq!(req.name, "fs_read_file");
            assert_eq!(req.arguments["path"], "foo.rs");
        }
        CommittedEvent::None => panic!("expected committed ToolCallRequest, got None"),
    }
}

/// Regression for the duplicate-render bug: a second `Flush` after the
/// tool-call buffer has already been drained must NOT re-fire dispatch.
/// The previous implementation inferred dispatch from the stream tail, which
/// kept pointing at the prior `ToolCallRequest`; the new contract drives
/// dispatch off `EventBuilder::handle_flush`, which is idempotent.
#[test]
fn duplicate_flush_after_tool_call_does_not_surface_request() {
    let mut stream = ConversationStream::new_test();
    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let mut coordinator = TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    );

    coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    coordinator.handle_event(&mut stream, Event::tool_call_start(2, "call_xyz", "run_me"));
    coordinator.handle_event(&mut stream, Event::tool_call_args(2, "{}"));

    // First flush legitimately produces the tool call.
    let first = coordinator.handle_event(&mut stream, Event::flush(2));
    assert!(
        matches!(first.action, Action::Continue),
        "first flush should continue the stream, got {first:?}"
    );
    assert!(
        matches!(first.committed, CommittedEvent::ToolCallRequest(_)),
        "first flush should surface the request, got {first:?}"
    );

    // Second flush of the same index — the kind a misbehaving provider
    // would emit — must be a no-op for dispatch. The stream tail is
    // still a `ToolCallRequest`, but that's no longer the dispatch
    // signal.
    let second = coordinator.handle_event(&mut stream, Event::flush(2));
    assert!(
        matches!(second.action, Action::Continue),
        "duplicate flush should continue the stream, got {second:?}"
    );
    assert!(
        matches!(second.committed, CommittedEvent::None),
        "duplicate flush must NOT re-dispatch, got {second:?}"
    );

    // And the stream still has exactly one tool-call request — the
    // second flush did not append anything.
    let tool_calls = stream
        .iter()
        .filter(|e| e.event.as_tool_call_request().is_some())
        .count();
    assert_eq!(tool_calls, 1);
}
