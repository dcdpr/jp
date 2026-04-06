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

use std::{fmt::Write as _, sync::Arc};

use jp_config::assistant::request::RequestConfig;
use jp_conversation::event::ChatResponse;
use jp_llm::{StreamError, exponential_backoff};
use jp_printer::Printer;
use jp_workspace::ConversationMut;
use tracing::{error, warn};

use crate::{
    cmd::query::{interrupt::LoopAction, turn::TurnCoordinator},
    error::Error,
};

/// Tracks retry state for stream errors within a single turn.
///
/// Counts consecutive stream failures and enforces retry limits from
/// [`RequestConfig`]. The counter resets when a new streaming cycle produces
/// its first successful event.
pub struct StreamRetryState {
    /// Retry configuration (max retries, backoff parameters).
    config: RequestConfig,

    /// Number of consecutive stream failures without a successful cycle.
    consecutive_failures: u32,

    /// Whether a temporary retry notification line is currently displayed.
    ///
    /// When `true`, the next retry or successful event should overwrite the
    /// line using `\r\x1b[K` rather than printing a new one.
    line_active: bool,

    /// Whether output is a TTY (enables temp-line rewriting).
    is_tty: bool,
}

impl StreamRetryState {
    /// Create a new retry state from the given configuration.
    pub fn new(config: RequestConfig, is_tty: bool) -> Self {
        Self {
            config,
            consecutive_failures: 0,
            line_active: false,
            is_tty,
        }
    }

    /// Reset the failure counter.
    ///
    /// Call this when the first successful LLM event arrives in a new streaming
    /// cycle. This ensures that partially successful streams (e.g. rate-limited
    /// mid-response) don't permanently consume the retry budget.
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Clear the retry notification line if one is currently displayed.
    ///
    /// Call this when the first successful event arrives, before rendering any
    /// LLM content.
    pub fn clear_line(&mut self, printer: &Printer) {
        if !self.line_active {
            return;
        }

        if self.is_tty {
            let _ = write!(printer.err_writer(), "\r\x1b[K");
        }

        self.line_active = false;
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

    /// Write the retry notification, overwriting any previous retry line on TTY
    /// or printing a new permanent line otherwise.
    fn notify(&mut self, kind: &str, printer: &Printer) {
        let attempt = self.consecutive_failures;
        let max = self.config.max_retries;
        let msg = format!("⚠ {kind}, retrying ({attempt}/{max})…");

        if self.is_tty {
            // Overwrite any previous retry line in-place.
            let _ = write!(printer.err_writer(), "\r\x1b[K{msg}");
            self.line_active = true;
        } else {
            printer.eprintln(msg);
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
    conv: &ConversationMut,
    printer: &Arc<Printer>,
) -> LoopAction<Result<(), Error>> {
    if !retry_state.can_retry(&error) {
        // Clear the temp line before printing the final error so it doesn't
        // linger on screen.
        retry_state.clear_line(printer);

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
        conv.update_events(|stream| {
            stream
                .current_turn_mut()
                .add_chat_response(ChatResponse::message(&content))
                .build()
                .expect("Invalid ConversationStream state");
        });
    }
    turn_coordinator.prepare_continuation();

    // 4. Notify the user.
    let attempt = retry_state.consecutive_failures;
    let max = retry_state.config.max_retries;
    let kind = error.kind.as_str();

    warn!(attempt, max, kind, "{error}");
    retry_state.notify(kind, printer);

    // 5. Backoff.
    let delay = retry_state.backoff_duration(&error);
    tokio::time::sleep(delay).await;

    LoopAction::Break
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
