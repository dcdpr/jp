use std::sync::Arc;

use assert_matches::assert_matches;
use jp_config::AppConfig;
use jp_conversation::{ConversationStream, event::ChatRequest};
use jp_inquire::{ReplyEditMode, ReplyOutcome, prompt::MockPromptBackend};
use jp_printer::{OutputFormat, Printer};

use super::*;

fn make_printer() -> Printer {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    printer
}

fn streaming_prompt() -> StreamingInterruptConfig {
    StreamingInterruptConfig::default()
}

fn tool_prompt() -> ToolInterruptConfig {
    ToolInterruptConfig::default()
}

fn make_turn_coordinator() -> TurnCoordinator {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    TurnCoordinator::new(
        Arc::new(printer),
        AppConfig::new_test().style,
        None,
        None,
        None,
    )
}

/// Regression: when the user picks `Continue` (`'c'`) while the LLM stream is
/// still alive, the action is `Resume` — "keep waiting for the current
/// stream."
/// The handler must return `LoopAction::Continue` so the existing `SelectAll`
/// (and the in-flight HTTP stream) stays alive.
/// Returning `Break` here drops the current stream and forces a redundant new
/// request, which can land us in inconsistent state and was the root of the
/// `tool_use without tool_result` follow-up failures.
#[test]
fn streaming_interrupt_resume_continues_without_breaking_loop() {
    let printer = make_printer();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // 'c' chosen while the stream is alive maps to InterruptAction::Resume.
    let backend = MockPromptBackend::new().with_inline_responses(['c']);

    let result = handle_streaming_interrupt(
        &mut turn_coordinator,
        &mut stream,
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &streaming_prompt(),
        false, // stream NOT finished -> stream alive -> Resume path
    );

    assert!(
        matches!(result, StreamingInterruptResult::Continue),
        "Resume must return Continue (not Break) so the current stream keeps polling; got \
         {result:?}"
    );
    assert_eq!(
        turn_coordinator.current_phase(),
        TurnPhase::Streaming,
        "Resume must leave the phase as Streaming"
    );
}

/// When the stream has already finished by the time the menu opens, `'c'` maps
/// to `Continue` (prefill path).
/// That path needs to break the inner loop so the outer turn loop issues a
/// fresh request with the partial content as prefill.
#[test]
fn streaming_interrupt_continue_breaks_for_prefill_request() {
    let printer = make_printer();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    let backend = MockPromptBackend::new().with_inline_responses(['c']);

    let result = handle_streaming_interrupt(
        &mut turn_coordinator,
        &mut stream,
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &streaming_prompt(),
        true, // stream finished -> dead -> Continue path
    );

    assert!(
        matches!(result, StreamingInterruptResult::Break),
        "Continue (prefill) must Break so the outer loop issues the next request; got {result:?}"
    );
}

#[test]
fn streaming_interrupt_menu_cancel_escalates() {
    let printer = make_printer();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Unflushed partial content sits in the event builder.
    turn_coordinator.handle_event(&mut stream, Event::message(0, "partial answer"));
    let len_before = stream.len();

    // No pre-loaded responses: the menu select is cancelled, as a Ctrl-C
    // press while the menu is showing would be.
    let backend = MockPromptBackend::new();

    let result = handle_streaming_interrupt(
        &mut turn_coordinator,
        &mut stream,
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &streaming_prompt(),
        false,
    );

    assert_eq!(result, StreamingInterruptResult::Escalate);
    // The partial content was committed and the turn completed.
    assert_eq!(stream.len(), len_before + 1);
    assert_eq!(turn_coordinator.current_phase(), TurnPhase::Complete);
}

#[test]
fn tool_interrupt_restart_returns_restart() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 't' (Restart) from interrupt menu
    let backend = MockPromptBackend::new().with_inline_responses(['t']);

    let result = handle_tool_interrupt(
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &tool_prompt(),
    );

    assert_eq!(result, ToolInterruptResult::Restart);
    assert!(
        token.is_cancelled(),
        "Restart should cancel current execution"
    );
}

#[test]
fn tool_interrupt_cancelled_returns_cancelled_with_canned_response() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 'r' (Stop & respond) then submitting empty — canned
    // response.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit(String::new())]);

    let result = handle_tool_interrupt(
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &tool_prompt(),
    );

    assert_matches!(
        result, ToolInterruptResult::Cancelled { ref response } if response.contains("intentionally rejected"),
        "Expected Cancelled with canned response, got {result:?}",
    );
    assert!(token.is_cancelled(), "Cancel should stop current execution");
}

#[test]
fn tool_interrupt_cancelled_with_custom_response() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 'r' (Stop & respond) then typing a message
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit("wrong tool, use grep instead".into())]);

    let result = handle_tool_interrupt(
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &tool_prompt(),
    );

    assert_eq!(result, ToolInterruptResult::Cancelled {
        response: "wrong tool, use grep instead".into()
    });
    assert!(token.is_cancelled(), "Cancel should stop current execution");
}

#[test]
fn tool_interrupt_resume_continues_without_cancel() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 'c' (Continue/wait for tool) from interrupt menu
    let backend = MockPromptBackend::new().with_inline_responses(['c']);

    let result = handle_tool_interrupt(
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &tool_prompt(),
    );

    assert_eq!(result, ToolInterruptResult::Continue);
    assert!(
        !token.is_cancelled(),
        "Resume should NOT cancel - tool continues running"
    );
}

#[test]
fn tool_interrupt_declined_when_prompting() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // The menu must NOT be shown while a tool prompt is active; the
    // notification is declined so it can propagate down the handler stack.
    let backend = MockPromptBackend::new().with_inline_responses(['r']);

    let result = handle_tool_interrupt(
        &token,
        &mut turn_coordinator,
        true, // prompting
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &tool_prompt(),
    );

    assert_eq!(result, ToolInterruptResult::Declined);
    assert!(
        !token.is_cancelled(),
        "Should NOT cancel when a prompt is active"
    );
}

#[test]
fn tool_interrupt_handled_when_not_prompting() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // This should show the interrupt menu; an empty reply cancels the tool.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit(String::new())]);

    let result = handle_tool_interrupt(
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &tool_prompt(),
    );

    // Should process the interrupt and cancel
    assert!(
        matches!(result, ToolInterruptResult::Cancelled { .. }),
        "Expected Cancelled variant when not prompting, got {result:?}"
    );
    assert!(
        token.is_cancelled(),
        "Should cancel when no prompt is active"
    );
}

#[test]
fn tool_interrupt_menu_cancel_escalates() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // No pre-loaded responses: the menu select is cancelled, as a Ctrl-C
    // press while the menu is showing would be.
    let backend = MockPromptBackend::new();

    let result = handle_tool_interrupt(
        &token,
        &mut turn_coordinator,
        false, // not prompting
        &printer,
        &backend,
        None,
        ReplyEditMode::Emacs,
        &tool_prompt(),
    );

    assert_eq!(result, ToolInterruptResult::Escalate);
    assert!(
        token.is_cancelled(),
        "Escalation should cancel the running tools"
    );
}
