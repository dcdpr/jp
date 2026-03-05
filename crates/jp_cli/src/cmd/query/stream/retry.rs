//! Unified stream retry logic.
//!
//! This module is the **single source of truth** for handling retryable
//! stream errors during LLM streaming. It consolidates retry decisions,
//! backoff, user notification, and state flushing into one place.
//!
//! # Error Classification
//!
//! Error classification is owned by [`StreamError::is_retryable`] in
//! `jp_llm`. This module only makes the retry *decision* based on that
//! classification and the current retry budget.
//!
//! # Retry Flow
//!
//! When a retryable error occurs during streaming:
//!
//! 1. Flush any partial (unflushed) content to the `ConversationStream`
//! 2. Reset the `TurnCoordinator` for a new streaming cycle
//! 3. Print a retry notification to the terminal
//! 4. Sleep for the backoff duration
//! 5. Break the inner event loop — the outer turn loop re-enters
//!    `TurnPhase::Streaming`, rebuilds the thread (which now includes the
//!    flushed content), and creates a fresh stream
//!
//! [`StreamError::is_retryable`]: jp_llm::StreamError::is_retryable

use std::sync::Arc;

use jp_config::assistant::request::RequestConfig;
use jp_conversation::ConversationStream;
use jp_llm::{StreamError, exponential_backoff};
use jp_printer::Printer;
use tracing::{error, warn};

use crate::{
    cmd::query::{interrupt::LoopAction, turn::TurnCoordinator},
    error::Error,
};

/// Tracks retry state for stream errors within a single turn.
///
/// Counts consecutive stream failures and enforces retry limits from
/// [`RequestConfig`]. The counter resets when a streaming cycle completes
/// successfully (i.e., `Event::Finished` is received).
pub struct StreamRetryState {
    /// Retry configuration (max retries, backoff parameters).
    config: RequestConfig,

    /// Number of consecutive stream failures without a successful cycle.
    consecutive_failures: u32,
}

impl StreamRetryState {
    /// Create a new retry state from the given configuration.
    pub fn new(config: RequestConfig) -> Self {
        Self {
            config,
            consecutive_failures: 0,
        }
    }

    /// Reset the failure counter after a successful streaming cycle.
    ///
    /// Call this when `Event::Finished` is received, indicating the stream
    /// completed without error.
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Check whether we should retry the given error.
    fn can_retry(&self, error: &StreamError) -> bool {
        error.is_retryable() && self.consecutive_failures < self.config.max_retries
    }

    /// Record a retry attempt. Must be called before sleeping.
    fn record_attempt(&mut self) {
        self.consecutive_failures += 1;
    }

    /// Compute the backoff duration for the current attempt.
    ///
    /// Uses the provider-specified `retry_after` if available (capped at
    /// `max_backoff_secs`), otherwise falls back to exponential backoff.
    fn backoff_duration(&self, error: &StreamError) -> std::time::Duration {
        let max = std::time::Duration::from_secs(u64::from(self.config.max_backoff_secs));

        match error.retry_after {
            Some(d) => d.min(max),
            None => exponential_backoff(
                self.consecutive_failures,
                u64::from(self.config.base_backoff_ms),
                u64::from(self.config.max_backoff_secs),
            ),
        }
    }
}

/// Single source of truth for handling stream errors during LLM streaming.
///
/// Decides whether to retry, flushes state, notifies the user, and sleeps
/// for the backoff duration. Returns a [`LoopAction`] telling the caller
/// what to do next.
///
/// # Returns
///
/// - [`LoopAction::Break`] — retryable error within budget. The caller
///   should break the inner event loop; the outer turn loop will re-enter
///   `TurnPhase::Streaming` with a fresh stream.
/// - [`LoopAction::Return`] — non-retryable error or retry budget
///   exhausted. The caller should propagate the error.
pub async fn handle_stream_error(
    error: StreamError,
    retry_state: &mut StreamRetryState,
    turn_coordinator: &mut TurnCoordinator,
    conversation_stream: &mut ConversationStream,
    printer: &Arc<Printer>,
) -> LoopAction<Result<(), Error>> {
    if !retry_state.can_retry(&error) {
        error!("Stream error (not retryable or max retries exceeded): {error}");
        return LoopAction::Return(Err(jp_llm::Error::Stream(error).into()));
    }

    // 1. Record the attempt (must happen before backoff calculation).
    retry_state.record_attempt();

    // 2. Flush renderer so any buffered markdown is queued to the printer,
    //    then drain the printer instantly so content is visible.
    turn_coordinator.flush_renderer();
    printer.flush_instant();

    // 3. Flush any unflushed partial content to the ConversationStream so
    //    it will be included when the thread is rebuilt for the retry.
    if let Some(content) = turn_coordinator.peek_partial_content() {
        conversation_stream
            .add_chat_response(jp_conversation::event::ChatResponse::message(&content));
    }
    turn_coordinator.prepare_continuation();

    // 4. Notify the user.
    let attempt = retry_state.consecutive_failures;
    let max = retry_state.config.max_retries;
    let kind = error.kind.as_str();

    warn!(attempt, max, kind, "{error}");
    printer.println(format!("⚠ {kind}, retrying ({attempt}/{max})…",));

    // 5. Backoff.
    let delay = retry_state.backoff_duration(&error);
    tokio::time::sleep(delay).await;

    LoopAction::Break
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
