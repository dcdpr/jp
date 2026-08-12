use std::sync::Arc;

use assert_matches::assert_matches;
use jp_config::{
    AppConfig,
    assistant::request::{CachePolicy, RequestConfig},
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse},
};
use jp_inquire::{ReplyEditMode, ReplyOutcome, prompt::MockPromptBackend};
use jp_llm::event::{EventMatcher, EventPatch, FinishReason, PatchAction};
use jp_printer::{OutputFormat, Printer};

use super::*;
use crate::cmd::query::stream::retry::MAX_CONSECUTIVE_REBUILDS;

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

fn make_retry_state(max_retries: u32) -> StreamRetryState {
    let config = RequestConfig {
        max_retries,
        base_backoff_ms: 1,
        max_backoff_secs: 1,
        stream_idle_timeout_secs: 120,
        max_response_bytes: 1_048_576,
        cache: CachePolicy::default(),
    };
    StreamRetryState::new(config, false)
}

/// The metadata a provider patch targets in these tests.
const PATCH_KEY: &str = "provider_signature";
const PATCH_VALUE: &str = "stale";

/// A stream holding one reasoning event that carries patchable metadata.
fn stream_with_patchable_event() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    let mut event = ConversationEvent::now(ChatResponse::reasoning("thinking"));
    event.metadata.insert(PATCH_KEY.into(), PATCH_VALUE.into());

    stream
        .current_turn_mut()
        .add_event(event)
        .build()
        .expect("valid stream");

    stream
}

fn stale_signature_patch() -> Event {
    Event::Patch(vec![EventPatch {
        matcher: EventMatcher::MetadataValue {
            key: PATCH_KEY.into(),
            value: PATCH_VALUE.into(),
        },
        action: PatchAction::RemoveMetadata(PATCH_KEY.into()),
    }])
}

/// A patch set targeting metadata no event carries, so applying it changes
/// nothing.
fn no_op_patch() -> Event {
    Event::Patch(vec![EventPatch {
        matcher: EventMatcher::MetadataValue {
            key: "absent_key".into(),
            value: "absent_value".into(),
        },
        action: PatchAction::RemoveMetadata("absent_key".into()),
    }])
}

/// The event contract allows several patch sets before the rebuild request, so
/// their outcomes accumulate: one set that changed the stream is enough to make
/// the rebuilt request differ, whatever the sets around it did.
#[test]
fn a_later_no_op_patch_does_not_erase_an_earlier_effective_one() {
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = stream_with_patchable_event();
    let mut retry_state = make_retry_state(2);

    handle_llm_event(
        stale_signature_patch(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );
    handle_llm_event(
        no_op_patch(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    let (action, _) = handle_llm_event(
        Event::Finished(FinishReason::Retry),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    assert_matches!(action, LoopAction::Break);
}

/// A patch that changed the stream earns the rebuild that follows it: the
/// request the provider gets next is genuinely different.
#[test]
fn rebuild_allowed_after_a_patch_changed_the_stream() {
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = stream_with_patchable_event();
    let mut retry_state = make_retry_state(2);

    let (patch_action, _) = handle_llm_event(
        stale_signature_patch(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );
    assert_matches!(patch_action, LoopAction::Continue);

    let (action, _) = handle_llm_event(
        Event::Finished(FinishReason::Retry),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    assert_matches!(action, LoopAction::Break);
}

/// A patch that matched nothing leaves the request byte-identical, so resending
/// it can only fail the same way.
#[test]
fn rebuild_refused_when_the_patch_matched_nothing() {
    let mut turn_coordinator = make_turn_coordinator();
    // No event carries the targeted metadata, so the patch applies to nothing.
    let mut stream = ConversationStream::new_test();
    let mut retry_state = make_retry_state(2);

    handle_llm_event(
        stale_signature_patch(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    let (action, _) = handle_llm_event(
        Event::Finished(FinishReason::Retry),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    assert_matches!(
        action,
        LoopAction::RebuildRefused(RebuildRefusal::NoProgress)
    );
}

/// A rebuild request with no patch at all cannot change anything either.
#[test]
fn rebuild_refused_without_a_preceding_patch() {
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = stream_with_patchable_event();
    let mut retry_state = make_retry_state(2);

    let (action, _) = handle_llm_event(
        Event::Finished(FinishReason::Retry),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    assert_matches!(
        action,
        LoopAction::RebuildRefused(RebuildRefusal::NoProgress)
    );
}

/// This is what bounds the loop for a misbehaving provider: one applied patch
/// authorizes exactly one rebuild, so a provider that keeps asking without
/// patching again is stopped on its second request.
#[test]
fn one_patch_authorizes_only_one_rebuild() {
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = stream_with_patchable_event();
    let mut retry_state = make_retry_state(2);

    handle_llm_event(
        stale_signature_patch(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    let retry = || Event::Finished(FinishReason::Retry);
    let (first, _) = handle_llm_event(
        retry(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );
    let (second, _) = handle_llm_event(
        retry(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    assert_matches!(first, LoopAction::Break);
    assert_matches!(
        second,
        LoopAction::RebuildRefused(RebuildRefusal::NoProgress)
    );
}

/// A repair that keeps making progress is still capped, so a provider whose
/// rejection carries no position (Google strips one signature per round) cannot
/// walk a long conversation at the cost of a full request per step.
#[test]
fn rebuild_refused_past_the_consecutive_limit() {
    let mut turn_coordinator = make_turn_coordinator();
    let mut retry_state = make_retry_state(2);

    // Every round patches a fresh stream, so every round makes progress and the
    // only thing that can stop the loop is the cap.
    let mut actions = vec![];
    for _ in 0..=MAX_CONSECUTIVE_REBUILDS {
        let mut stream = stream_with_patchable_event();

        handle_llm_event(
            stale_signature_patch(),
            &mut turn_coordinator,
            &mut stream,
            &mut retry_state,
        );

        let (action, _) = handle_llm_event(
            Event::Finished(FinishReason::Retry),
            &mut turn_coordinator,
            &mut stream,
            &mut retry_state,
        );
        actions.push(action);
    }

    let (last, allowed) = actions.split_last().expect("at least one round");

    assert_eq!(allowed.len(), MAX_CONSECUTIVE_REBUILDS as usize);
    for (round, action) in allowed.iter().enumerate() {
        assert_matches!(action, LoopAction::Break, "round {round} is allowed");
    }

    assert_matches!(
        last,
        LoopAction::RebuildRefused(RebuildRefusal::LimitReached {
            limit: MAX_CONSECUTIVE_REBUILDS
        })
    );
}

/// Streaming content again clears the cap, so a turn that repairs, answers, and
/// later needs an unrelated repair is not punished for the first one.
#[test]
fn successful_cycle_restores_the_rebuild_allowance() {
    let mut turn_coordinator = make_turn_coordinator();
    let mut retry_state = make_retry_state(2);

    for _ in 0..MAX_CONSECUTIVE_REBUILDS {
        let mut stream = stream_with_patchable_event();
        handle_llm_event(
            stale_signature_patch(),
            &mut turn_coordinator,
            &mut stream,
            &mut retry_state,
        );
        handle_llm_event(
            Event::Finished(FinishReason::Retry),
            &mut turn_coordinator,
            &mut stream,
            &mut retry_state,
        );
    }

    retry_state.reset();

    let mut stream = stream_with_patchable_event();
    handle_llm_event(
        stale_signature_patch(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );
    let (action, _) = handle_llm_event(
        Event::Finished(FinishReason::Retry),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    assert_matches!(action, LoopAction::Break);
}

/// A cycle that streams content again drops any pending repair, so a later
/// unrelated rebuild request cannot ride on a patch from before it.
#[test]
fn successful_cycle_drops_the_pending_patch() {
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = stream_with_patchable_event();
    let mut retry_state = make_retry_state(2);

    handle_llm_event(
        stale_signature_patch(),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );
    retry_state.reset();

    let (action, _) = handle_llm_event(
        Event::Finished(FinishReason::Retry),
        &mut turn_coordinator,
        &mut stream,
        &mut retry_state,
    );

    assert_matches!(
        action,
        LoopAction::RebuildRefused(RebuildRefusal::NoProgress)
    );
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
/// to `Continue`.
/// That path needs to break the inner loop so the outer turn loop issues a
/// fresh request with the partial response as continuation context.
#[test]
fn streaming_interrupt_continue_breaks_for_continuation_request() {
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
        "Continue must Break so the outer loop issues the next request; got {result:?}"
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
fn tool_interrupt_cancelled_empty_reply_has_no_custom_message() {
    let printer = make_printer();
    let token = CancellationToken::new();
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Mock user selecting 'r' (Stop & respond) then submitting empty — no
    // custom message; the coordinator fills in each tool's configured
    // cancellation response.
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
        result,
        ToolInterruptResult::Cancelled {
            response: None,
            exit: false
        },
        "Expected Cancelled without a custom message, got {result:?}",
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
        response: Some("wrong tool, use grep instead".into()),
        exit: false
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
