use std::{sync::Arc, time::Duration};

use jp_config::{AppConfig, assistant::request::RequestConfig};
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ChatResponse, ConversationEvent},
};
use jp_llm::{StreamError, event::Event};
use jp_printer::{OutputFormat, Printer};

use super::*;
use crate::cmd::query::interrupt::LoopAction;

fn make_retry_state(max_retries: u32) -> StreamRetryState {
    let config = RequestConfig {
        max_retries,
        base_backoff_ms: 1, // 1ms for fast tests
        max_backoff_secs: 1,
    };
    StreamRetryState::new(config)
}

fn make_turn_coordinator() -> TurnCoordinator {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    TurnCoordinator::new(Arc::new(printer), AppConfig::new_test().style)
}

#[test]
fn can_retry_retryable_within_budget() {
    let state = make_retry_state(3);
    assert!(state.can_retry(&StreamError::transient("test")));
}

#[test]
fn cannot_retry_non_retryable() {
    let state = make_retry_state(3);
    assert!(!state.can_retry(&StreamError::other("test")));
}

#[test]
fn cannot_retry_when_budget_exhausted() {
    let mut state = make_retry_state(2);
    state.record_attempt();
    state.record_attempt();
    assert!(!state.can_retry(&StreamError::transient("test")));
}

#[test]
fn reset_clears_failure_count() {
    let mut state = make_retry_state(2);
    state.record_attempt();
    state.record_attempt();
    assert!(!state.can_retry(&StreamError::transient("test")));

    state.reset();
    assert!(state.can_retry(&StreamError::transient("test")));
}

#[test]
fn backoff_uses_retry_after_when_present() {
    let config = RequestConfig {
        max_retries: 3,
        base_backoff_ms: 1,
        max_backoff_secs: 120,
    };
    let state = StreamRetryState::new(config);
    let err = StreamError::rate_limit(Some(Duration::from_secs(42)));
    assert_eq!(state.backoff_duration(&err), Duration::from_secs(42));
}

#[test]
fn backoff_caps_retry_after_at_max_backoff() {
    let state = make_retry_state(3); // max_backoff_secs = 1
    let err = StreamError::rate_limit(Some(Duration::from_mins(5)));
    assert_eq!(state.backoff_duration(&err), Duration::from_secs(1));
}

#[test]
fn backoff_uses_exponential_when_no_retry_after() {
    let mut state = make_retry_state(3);
    state.record_attempt();
    let err = StreamError::transient("test");
    let duration = state.backoff_duration(&err);
    // Should be > 0 (base_backoff_ms=1, attempt=1)
    assert!(duration.as_millis() > 0);
}

#[tokio::test]
async fn retryable_error_breaks_for_retry() {
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    let error = StreamError::transient("server overloaded");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &mut stream,
        &printer,
    )
    .await;

    assert!(matches!(result, LoopAction::Break));
    assert_eq!(retry_state.consecutive_failures, 1);

    // Should have printed a retry notification
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("Server error, retrying"),
        "Should notify user. Output: {output}"
    );
}

#[tokio::test]
async fn non_retryable_error_returns_error() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();

    let error = StreamError::other("auth failure");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &mut stream,
        &printer,
    )
    .await;

    assert!(matches!(result, LoopAction::Return(Err(_))));
    assert_eq!(retry_state.consecutive_failures, 0); // not incremented
}

#[tokio::test]
async fn budget_exhausted_returns_error() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(1);
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();

    // First attempt exhausts budget
    retry_state.record_attempt();

    let error = StreamError::transient("still broken");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &mut stream,
        &printer,
    )
    .await;

    assert!(matches!(result, LoopAction::Return(Err(_))));
}

#[tokio::test]
async fn partial_content_flushed_on_retry() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // Simulate partial content accumulated in the event builder
    turn_coordinator.handle_event(&mut stream, Event::Part {
        index: 0,
        event: ConversationEvent::now(ChatResponse::message("Hello ")),
    });
    turn_coordinator.handle_event(&mut stream, Event::Part {
        index: 0,
        event: ConversationEvent::now(ChatResponse::message("world")),
    });

    // Verify partial content exists before the error
    assert_eq!(
        turn_coordinator.peek_partial_content(),
        Some("Hello world".to_string())
    );

    let error = StreamError::connect("connection reset");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &mut stream,
        &printer,
    )
    .await;

    assert!(matches!(result, LoopAction::Break));

    // Partial content should have been flushed to the conversation stream
    let has_response = stream.iter().any(|e| {
        e.event.as_chat_response().is_some_and(
            |r| matches!(r, ChatResponse::Message { message } if message == "Hello world"),
        )
    });
    assert!(
        has_response,
        "Partial content should be flushed to ConversationStream"
    );

    // TurnCoordinator should be reset for a new cycle
    assert_eq!(turn_coordinator.peek_partial_content(), None);
}

#[tokio::test]
async fn retry_without_partial_content_still_works() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let mut stream = ConversationStream::new_test();
    turn_coordinator.start_turn(&mut stream, ChatRequest::from("test"));

    // No partial content — error happens before any events
    assert_eq!(turn_coordinator.peek_partial_content(), None);

    let error = StreamError::transient("503 Service Unavailable");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &mut stream,
        &printer,
    )
    .await;

    // Should still break for retry, even without partial content
    assert!(
        matches!(result, LoopAction::Break),
        "Should retry even without partial content"
    );
}
