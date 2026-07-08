use std::{sync::Arc, time::Duration};

use jp_config::{
    AppConfig,
    assistant::request::{CachePolicy, RequestConfig},
};
use jp_conversation::{
    Conversation,
    event::{ChatRequest, ChatResponse},
};
use jp_llm::{StreamError, event::Event};
use jp_printer::{OutputFormat, Printer};
use jp_workspace::{ConversationLock, Workspace};

use super::*;
use crate::signals::testing::{detached_router, test_router};

fn make_retry_state(max_retries: u32) -> StreamRetryState {
    let config = RequestConfig {
        max_retries,
        base_backoff_ms: 1, // 1ms for fast tests
        max_backoff_secs: 1,
        stream_idle_timeout_secs: 120,
        cache: CachePolicy::default(),
    };
    StreamRetryState::new(config, false)
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

/// Create a workspace with a single conversation and return a test lock.
fn make_test_lock() -> (Workspace, ConversationLock) {
    let config = Arc::new(AppConfig::new_test());
    let mut workspace = Workspace::new(camino::Utf8PathBuf::new());
    let id = workspace.create_conversation(Conversation::default(), config);
    let handle = workspace.acquire_conversation(&id).unwrap();
    let lock = workspace.test_lock(handle);
    (workspace, lock)
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
        stream_idle_timeout_secs: 120,
        cache: CachePolicy::default(),
    };
    let state = StreamRetryState::new(config, false);
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
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let (_ws, lock) = make_test_lock();
    let conv = lock.as_mut();
    conv.update_events(|stream| {
        turn_coordinator.start_turn(stream, ChatRequest::from("test"));
    });

    let router = detached_router();
    let error = StreamError::transient("server overloaded");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &conv,
        &printer,
        &router,
    )
    .await;

    assert!(matches!(result, StreamErrorOutcome::Retry));
    assert_eq!(retry_state.consecutive_failures, 1);

    // Should have printed a retry notification to stderr
    printer.flush();
    let output = err.lock();
    assert!(
        output.contains("Server error, retrying"),
        "Should notify user on stderr. Output: {output}"
    );
}

#[tokio::test]
async fn non_retryable_error_returns_error() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let (_ws, lock) = make_test_lock();
    let conv = lock.as_mut();

    let router = detached_router();
    let error = StreamError::other("auth failure");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &conv,
        &printer,
        &router,
    )
    .await;

    assert!(matches!(result, StreamErrorOutcome::Fatal(_)));
    assert_eq!(retry_state.consecutive_failures, 0); // not incremented
}

#[tokio::test]
async fn budget_exhausted_returns_error() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(1);
    let mut turn_coordinator = make_turn_coordinator();
    let (_ws, lock) = make_test_lock();
    let conv = lock.as_mut();

    // First attempt exhausts budget
    retry_state.record_attempt();

    let router = detached_router();
    let error = StreamError::transient("still broken");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &conv,
        &printer,
        &router,
    )
    .await;

    assert!(matches!(result, StreamErrorOutcome::Fatal(_)));
}

#[tokio::test]
async fn partial_content_flushed_on_retry() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let (_ws, lock) = make_test_lock();
    let conv = lock.as_mut();
    conv.update_events(|stream| {
        turn_coordinator.start_turn(stream, ChatRequest::from("test"));
    });

    // Simulate partial content accumulated in the event builder
    conv.update_events(|stream| {
        turn_coordinator.handle_event(stream, Event::message(0, "Hello "));
        turn_coordinator.handle_event(stream, Event::message(0, "world"));
    });

    // Verify partial content exists before the error
    let partial = turn_coordinator.peek_partial_events();
    assert_eq!(partial.len(), 1);
    assert!(
        matches!(&partial[0], ChatResponse::Message { message } if message == "Hello world"),
        "got {partial:?}"
    );

    let router = detached_router();
    let error = StreamError::connect("connection reset");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &conv,
        &printer,
        &router,
    )
    .await;

    assert!(matches!(result, StreamErrorOutcome::Retry));

    // Partial content should have been flushed to the conversation stream
    let has_response = conv.events().iter().any(|e| {
        e.event.as_chat_response().is_some_and(
            |r| matches!(r, ChatResponse::Message { message } if message == "Hello world"),
        )
    });
    assert!(
        has_response,
        "Partial content should be flushed to ConversationStream"
    );

    // TurnCoordinator should be reset for a new cycle
    assert!(turn_coordinator.peek_partial_events().is_empty());
}

#[tokio::test]
async fn partial_content_flushed_on_abort() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let (_ws, lock) = make_test_lock();
    let conv = lock.as_mut();
    conv.update_events(|stream| {
        turn_coordinator.start_turn(stream, ChatRequest::from("test"));
    });

    conv.update_events(|stream| {
        turn_coordinator.handle_event(stream, Event::message(0, "Hello "));
        turn_coordinator.handle_event(stream, Event::message(0, "world"));
    });

    // A non-retryable error aborts the turn, but partial content must still be
    // flushed so streamed work isn't lost.
    let router = detached_router();
    let error = StreamError::other("auth failure");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &conv,
        &printer,
        &router,
    )
    .await;

    assert!(matches!(result, StreamErrorOutcome::Fatal(_)));

    let has_response = conv.events().iter().any(|e| {
        e.event.as_chat_response().is_some_and(
            |r| matches!(r, ChatResponse::Message { message } if message == "Hello world"),
        )
    });
    assert!(
        has_response,
        "Partial content should be flushed to ConversationStream even on abort"
    );
}

#[tokio::test]
async fn retry_without_partial_content_still_works() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut retry_state = make_retry_state(3);
    let mut turn_coordinator = make_turn_coordinator();
    let (_ws, lock) = make_test_lock();
    let conv = lock.as_mut();
    conv.update_events(|stream| {
        turn_coordinator.start_turn(stream, ChatRequest::from("test"));
    });

    // No partial content — error happens before any events
    assert!(turn_coordinator.peek_partial_events().is_empty());

    let router = detached_router();
    let error = StreamError::transient("503 Service Unavailable");
    let result = handle_stream_error(
        error,
        &mut retry_state,
        &mut turn_coordinator,
        &conv,
        &printer,
        &router,
    )
    .await;

    // Should still break for retry, even without partial content
    assert!(
        matches!(result, StreamErrorOutcome::Retry),
        "Should retry even without partial content"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupt_during_backoff_cuts_wait_short() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    // A provider-specified retry delay far longer than the test timeout: the
    // only way this test finishes quickly is the interrupt cutting it short.
    let config = RequestConfig {
        max_retries: 3,
        base_backoff_ms: 1,
        max_backoff_secs: 120,
        stream_idle_timeout_secs: 120,
        cache: CachePolicy::default(),
    };
    let mut retry_state = StreamRetryState::new(config, false);
    let mut turn_coordinator = make_turn_coordinator();
    let (_ws, lock) = make_test_lock();
    let conv = lock.as_mut();
    conv.update_events(|stream| {
        turn_coordinator.start_turn(stream, ChatRequest::from("test"));
    });

    let (router, signals) = test_router();
    let router = std::sync::Arc::new(router);
    // Unlike the turn-loop tests, there is no setup between spawning this
    // task and `handle_stream_error` registering its handler at entry, so a
    // short sleep reliably lands the press inside the backoff wait.
    let signal_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        signals.interrupt().await;
    });

    let error = StreamError::rate_limit(Some(Duration::from_mins(1)));
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        handle_stream_error(
            error,
            &mut retry_state,
            &mut turn_coordinator,
            &conv,
            &printer,
            &router,
        ),
    )
    .await
    .expect("the interrupt must cut the 60s backoff short");

    signal_handle.await.unwrap();

    assert!(matches!(result, StreamErrorOutcome::Interrupted));
    // The attempt was recorded before the wait began.
    assert_eq!(retry_state.consecutive_failures, 1);
}
