//! Unified stream retry logic.
//!
//! This module is the **single source of truth** for handling retryable stream
//! errors during LLM streaming.
//! It consolidates retry decisions, backoff, user notification, and state
//! flushing into one place.
//!
//! # Error Classification
//!
//! Error classification is owned by [`StreamError::is_retryable`] in `jp_llm`.
//! This module only makes the retry *decision* based on that classification and
//! the current retry budget.
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

use std::{fmt, fmt::Write as _, mem, sync::Arc};

use jp_config::assistant::request::RequestConfig;
use jp_llm::{StreamError, exponential_backoff};
use jp_printer::Printer;
use jp_workspace::ConversationMut;
use tracing::{error, warn};

use crate::{
    cmd::query::turn::TurnCoordinator,
    error::Error,
    signals::{InterruptNotice, SignalRouter},
};

/// How many provider-requested rebuilds a turn may attempt in a row.
///
/// Every rebuild re-sends the whole conversation, so a repair costs the round
/// count times the conversation size.
/// A repair needing more rounds than this is not worth paying for; the turn
/// reports the dead end instead.
///
/// Anthropic repairs a wedged turn in one round, and one round per affected
/// message after that.
/// Google's rejection carries no position, so its repair walks one signature
/// per round and a long conversation can exceed this, surfacing as an error
/// rather than a large bill.
///
/// The count resets once a cycle produces its first successful event, so a turn
/// that repairs, streams, and later repairs again gets a fresh allowance.
pub(crate) const MAX_CONSECUTIVE_REBUILDS: u32 = 10;

/// Why a provider-requested rebuild was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildRefusal {
    /// The preceding patch changed nothing, so the rebuilt request would be
    /// identical to the one the provider just rejected.
    NoProgress,

    /// The turn has rebuilt as many times in a row as it is allowed to.
    LimitReached {
        /// The allowance that was reached.
        limit: u32,
    },
}

impl fmt::Display for RebuildRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProgress => f.write_str(
                "the provider asked to rebuild the request but sent no fix that changed it, so \
                 resending would fail the same way",
            ),
            Self::LimitReached { limit } => write!(
                f,
                "the provider asked to rebuild the request {limit} times in a row without \
                 producing a usable response"
            ),
        }
    }
}

/// Tracks retry state for stream errors within a single turn.
///
/// Counts consecutive stream failures and enforces retry limits from
/// [`RequestConfig`].
/// The counter resets when a new streaming cycle produces its first successful
/// event.
pub struct StreamRetryState {
    /// Retry configuration (max retries, backoff parameters).
    config: RequestConfig,

    /// Number of consecutive stream failures without a successful cycle.
    consecutive_failures: u32,

    /// Whether any provider patch set in this cycle changed the conversation
    /// stream.
    patch_changed_stream: bool,

    /// Whether every provider patch set in this cycle strictly shrinks the
    /// stream.
    ///
    /// Accumulated rather than replaced, so a later set that shrinks cannot
    /// erase an earlier one that may not.
    patch_sets_shrink: bool,

    /// Number of consecutive provider-requested rebuilds without a successful
    /// cycle.
    consecutive_rebuilds: u32,

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
            patch_changed_stream: false,
            patch_sets_shrink: true,
            consecutive_rebuilds: 0,
            line_active: false,
            is_tty,
        }
    }

    /// Reset the failure counter.
    ///
    /// Call this when the first successful LLM event arrives in a new streaming
    /// cycle.
    /// This ensures that partially successful streams (e.g. rate-limited
    /// mid-response) don't permanently consume the retry budget.
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.patch_changed_stream = false;
        self.patch_sets_shrink = true;
        self.consecutive_rebuilds = 0;
    }

    /// Record the outcome of one provider patch set.
    ///
    /// `applied` is how many events it changed, and `shrinks` whether every
    /// action in the set strictly shrinks the stream (see
    /// [`PatchAction::shrinks_stream`]).
    ///
    /// Outcomes accumulate until a rebuild consumes them, because a provider
    /// may send several patch sets before asking for the rebuild: the rebuilt
    /// request differs if any set changed the stream, and the loop only
    /// terminates if every set shrinks it.
    ///
    /// [`PatchAction::shrinks_stream`]: jp_llm::event::PatchAction::shrinks_stream
    pub fn record_patch(&mut self, applied: usize, shrinks: bool) {
        self.patch_changed_stream |= applied > 0;
        self.patch_sets_shrink &= shrinks;
    }

    /// Authorize a provider-requested rebuild, or explain why it is refused.
    ///
    /// A provider that patches the conversation stream and asks for a rebuild
    /// reports no error, so these attempts never reach [`handle_stream_error`]
    /// and consume no part of the stream-error budget.
    /// They are bounded two ways: each rebuild must follow a patch that made
    /// progress, which is what guarantees the loop ends, and the number in a
    /// row is capped, which keeps a terminating-but-expensive repair from
    /// running up a bill.
    ///
    /// Consumes the accumulated patch record, so one patch cannot authorize two
    /// rebuilds.
    pub fn authorize_rebuild(&mut self) -> Result<(), RebuildRefusal> {
        let changed = mem::take(&mut self.patch_changed_stream);
        let shrinks = mem::replace(&mut self.patch_sets_shrink, true);

        if !(changed && shrinks) {
            return Err(RebuildRefusal::NoProgress);
        }

        self.consecutive_rebuilds += 1;
        if self.consecutive_rebuilds > MAX_CONSECUTIVE_REBUILDS {
            return Err(RebuildRefusal::LimitReached {
                limit: MAX_CONSECUTIVE_REBUILDS,
            });
        }

        Ok(())
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

    /// Record a retry attempt.
    /// Must be called before sleeping.
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

/// Whether the assistant's response continues past a flush boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseBoundary {
    /// The request is resent and the same response continues on screen, with
    /// nothing persistent rendered in between.
    Continuation,

    /// The response ends here: nothing more of it will be rendered.
    Final,
}

/// Flush buffered output and commit any unflushed partial assistant content to
/// the conversation stream.
///
/// Content sits in the coordinator's event builder until a flush or terminal
/// event reaches it, so persisting the conversation alone would drop text the
/// user already saw on screen.
/// Call this before ending a turn on any path that bypasses the coordinator's
/// own terminal handling.
///
/// `boundary` decides how the renderer treats an open reasoning region.
/// A continuation keeps the gap the region owes pending for the resent
/// request's output to resolve (the retry notification is a transient line that
/// leaves nothing behind); a final boundary closes the region with an unshaded
/// gap.
pub fn commit_partial_response(
    turn_coordinator: &mut TurnCoordinator,
    conv: &ConversationMut,
    printer: &Arc<Printer>,
    boundary: ResponseBoundary,
) {
    match boundary {
        ResponseBoundary::Continuation => turn_coordinator.flush_renderer_for_continuation(),
        ResponseBoundary::Final => turn_coordinator.flush_renderer(),
    }
    printer.flush_instant();

    let partial = turn_coordinator.peek_partial_events();
    if partial.is_empty() {
        return;
    }

    conv.update_events(|stream| {
        let mut turn = stream.current_turn_mut();
        for response in partial {
            turn = turn.add_chat_response(response);
        }
        turn.build().expect("Invalid ConversationStream state");
    });
}

/// Outcome of [`handle_stream_error`].
#[derive(Debug)]
pub enum StreamErrorOutcome {
    /// Retryable error within budget: break the inner event loop; the outer
    /// turn loop re-enters `TurnPhase::Streaming` with a fresh stream.
    Retry,

    /// Non-retryable error or retry budget exhausted: propagate.
    Fatal(Error),

    /// A Ctrl-C arrived during the backoff wait.
    /// The wait was cut short and the retry notification line cleared; the
    /// caller should run the streaming interrupt flow (the stream is dead).
    ///
    /// The press is carried out unresolved: only the caller, which runs the
    /// menu, knows whether it was answered.
    Interrupted(InterruptNotice),
}

/// Single source of truth for handling stream errors during LLM streaming.
///
/// Decides whether to retry, flushes state, notifies the user, and waits for
/// the backoff duration.
/// A Ctrl-C during the wait cuts it short and surfaces as
/// [`StreamErrorOutcome::Interrupted`] so the interrupt menu opens immediately
/// instead of after the wait.
pub async fn handle_stream_error(
    error: StreamError,
    retry_state: &mut StreamRetryState,
    turn_coordinator: &mut TurnCoordinator,
    conv: &ConversationMut,
    printer: &Arc<Printer>,
    signals: &SignalRouter,
) -> StreamErrorOutcome {
    // Always flush buffered renderer output and any unflushed partial content
    // to the stream BEFORE deciding whether to retry or abort. Streamed text
    // the user already saw must never be dropped just because the error turned
    // out to be fatal.
    // The retry decision does feed the flush: an aborted response ends its
    // reasoning region here, while a retry hands the region to the resent
    // request.
    let boundary = if retry_state.can_retry(&error) {
        ResponseBoundary::Continuation
    } else {
        ResponseBoundary::Final
    };
    commit_partial_response(turn_coordinator, conv, printer, boundary);

    if boundary == ResponseBoundary::Final {
        // Clear the temp line before printing the final error so it doesn't
        // linger on screen.
        retry_state.clear_line(printer);

        error!("Stream error (not retryable or max retries exceeded): {error}");
        return StreamErrorOutcome::Fatal(jp_llm::Error::Stream(error).into());
    }

    // Record the attempt (must happen before backoff calculation).
    retry_state.record_attempt();

    // Reset the coordinator for the next streaming cycle. The committed partial
    // response becomes continuation context in the rebuilt Thread; the Provider
    // decides how to encode it for the target model.
    turn_coordinator.prepare_retry_continuation();

    // Notify the user.
    let attempt = retry_state.consecutive_failures;
    let max = retry_state.config.max_retries;
    let kind = error.kind.as_str();

    warn!(attempt, max, kind, "{error}");
    retry_state.notify(kind, printer);

    // 5. Backoff. A Ctrl-C during the wait cuts it short: a temporary handler
    // scope (stacked above the streaming handler for the duration of the
    // wait) catches the press, so the caller can show the interrupt menu
    // immediately instead of after the wait.
    let delay = retry_state.backoff_duration(&error);
    let (interrupt_guard, mut interrupt_rx) = signals.push_handler();
    let notice = tokio::select! {
        biased;
        notice = interrupt_rx.recv() => notice,
        () = tokio::time::sleep(delay) => None,
    };
    drop(interrupt_guard);

    if let Some(notice) = notice {
        retry_state.clear_line(printer);
        return StreamErrorOutcome::Interrupted(notice);
    }

    StreamErrorOutcome::Retry
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
