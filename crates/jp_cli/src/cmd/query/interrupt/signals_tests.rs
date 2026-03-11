use std::sync::Arc;

use assert_matches::assert_matches;
use jp_config::AppConfig;
use jp_conversation::{ConversationStream, event::ChatRequest};
use jp_inquire::prompt::{MockPromptBackend, TerminalPromptBackend};
use jp_printer::{OutputFormat, Printer};

use super::*;

fn make_printer() -> Printer {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    printer
}

fn make_turn_coordinator() -> TurnCoordinator {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style)
}

#[test]
fn streaming_signal_quit_breaks_for_persist() {
    let printer = make_printer();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    let result = handle_streaming_signal(
        SignalTo::Quit,
        &mut turn_coordinator,
        &mut stream,
        &printer,
        &TerminalPromptBackend,
        false, // stream not finished
    );

    // Quit breaks (not returns) so persistence happens after the loop
    assert!(matches!(result, LoopAction::Break));
    assert_eq!(turn_coordinator.current_phase(), TurnPhase::Complete);
}

#[test]
fn tool_signal_quit_cancels_and_continues() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();

    let result = handle_tool_signal(
        SignalTo::Quit,
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &TerminalPromptBackend,
    );

    // Quit cancels tools and continues so normal persistence flow happens
    assert_eq!(result, ToolSignalResult::Continue);
    assert!(token.is_cancelled());
    assert_eq!(turn_coordinator.current_phase(), TurnPhase::Complete);
}

#[test]
fn regression_streaming_quit_must_not_skip_persistence() {
    // Regression test: Quit during streaming must NOT return early.
    // It must Break so the post-loop persist happens.
    let printer = make_printer();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    let result = handle_streaming_signal(
        SignalTo::Quit,
        &mut turn_coordinator,
        &mut stream,
        &printer,
        &TerminalPromptBackend,
        false, // stream not finished
    );

    assert!(
        matches!(result, LoopAction::Break),
        "Quit must return Break (not Return) to ensure persistence happens"
    );
}

#[test]
fn regression_tool_quit_must_not_skip_persistence() {
    // Regression test: Quit during tool execution must NOT return early.
    // It must Continue (after cancelling) so normal flow persists.
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    let result = handle_tool_signal(
        SignalTo::Quit,
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &TerminalPromptBackend,
    );

    assert!(
        matches!(result, ToolSignalResult::Continue),
        "Quit must return Continue (not Return) to ensure persistence happens"
    );
    assert!(
        token.is_cancelled(),
        "Quit must cancel tools to exit quickly"
    );
}

#[test]
fn tool_signal_shutdown_restart_returns_restart() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 'r' (Restart) from interrupt menu
    let backend = MockPromptBackend::new().with_inline_responses(['r']);

    let result = handle_tool_signal(
        SignalTo::Shutdown,
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
    );

    assert_eq!(result, ToolSignalResult::Restart);
    assert!(
        token.is_cancelled(),
        "Restart should cancel current execution"
    );
}

#[test]
fn tool_signal_shutdown_cancelled_returns_cancelled_with_canned_response() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 's' (Stop & Reply) with no text — canned response
    let backend = MockPromptBackend::new().with_inline_responses(['s']);

    let result = handle_tool_signal(
        SignalTo::Shutdown,
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
    );

    assert_matches!(
        result, ToolSignalResult::Cancelled { ref response } if response.contains("intentionally rejected"),
        "Expected Cancelled with canned response, got {result:?}",
    );
    assert!(token.is_cancelled(), "Cancel should stop current execution");
}

#[test]
fn tool_signal_shutdown_cancelled_with_custom_response() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 's' (Stop & Reply) then typing a message
    let backend = MockPromptBackend::new()
        .with_inline_responses(['s'])
        .with_text_responses(["wrong tool, use grep instead"]);

    let result = handle_tool_signal(
        SignalTo::Shutdown,
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
    );

    assert_eq!(result, ToolSignalResult::Cancelled {
        response: "wrong tool, use grep instead".into()
    });
    assert!(token.is_cancelled(), "Cancel should stop current execution");
}

#[test]
fn tool_signal_shutdown_resume_continues_without_cancel() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 'c' (Continue/wait for tool) from interrupt menu
    let backend = MockPromptBackend::new().with_inline_responses(['c']);

    let result = handle_tool_signal(
        SignalTo::Shutdown,
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
    );

    assert_eq!(result, ToolSignalResult::Continue);
    assert!(
        !token.is_cancelled(),
        "Resume should NOT cancel - tool continues running"
    );
}

#[test]
fn tool_signal_shutdown_suppressed_when_prompting() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // This should NOT show the interrupt menu because a prompt is active
    let backend = MockPromptBackend::new().with_inline_responses(['s']);

    let result = handle_tool_signal(
        SignalTo::Shutdown,
        &token,
        &mut turn_coordinator,
        true, // prompting
        &printer,
        &backend,
    );

    // Should continue without cancelling (prompt handles Ctrl+C)
    assert_eq!(result, ToolSignalResult::Continue);
    assert!(
        !token.is_cancelled(),
        "Should NOT cancel when prompt is active"
    );
}

#[test]
fn tool_signal_shutdown_not_suppressed_when_not_prompting() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // This should show the interrupt menu.
    let backend = MockPromptBackend::new().with_inline_responses(['s']);

    let result = handle_tool_signal(
        SignalTo::Shutdown,
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
    );

    // Should process the interrupt and cancel
    assert!(
        matches!(result, ToolSignalResult::Cancelled { .. }),
        "Expected Cancelled variant when not prompting, got {result:?}"
    );
    assert!(
        token.is_cancelled(),
        "Should cancel when no prompt is active"
    );
}
