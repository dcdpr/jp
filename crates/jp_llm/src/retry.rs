//! Retry utilities for resilient LLM request handling.

use std::time::Duration;

use futures::StreamExt as _;
use tracing::{debug, warn};

use crate::{
    Provider, StreamError, StreamErrorKind,
    error::Result,
    event::{Event, NoticeSink},
    model::ModelDetails,
    query::ChatQuery,
    stream::with_output_limit,
};

/// Configuration for resilient stream retries.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of transient retry attempts.
    /// Provider-confirmed credential changes do not consume this budget.
    pub max_retries: u32,

    /// Base backoff delay in milliseconds.
    pub base_backoff_ms: u64,

    /// Maximum backoff delay in seconds.
    pub max_backoff_secs: u64,

    /// Abort a response after it generates more than this many bytes.
    ///
    /// `None` leaves the response unbounded.
    pub max_response_bytes: Option<u64>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_backoff_ms: 1000,
            max_backoff_secs: 30,
            max_response_bytes: Some(1_048_576),
        }
    }
}

impl RetryConfig {
    /// Apply the user's configured output ceiling, leaving the retry and
    /// backoff settings at their defaults.
    ///
    /// The retry settings are deliberately not taken from the same config
    /// block: they are tuned for a query the user is watching, and a collect
    /// call that inherited a large `max_retries` would keep an unattended
    /// request alive far longer than the caller expects.
    #[must_use]
    pub fn with_max_response_bytes(mut self, max_response_bytes: Option<u64>) -> Self {
        self.max_response_bytes = max_response_bytes;
        self
    }
}

/// Execute `chat_completion_stream` with automatic retries on transient errors.
///
/// Collects the full event stream into a `Vec<Event>`.
/// On retryable stream errors, backs off and retries the entire request up to
/// `config.max_retries` times.
/// Provider-confirmed credential changes resubmit immediately without using
/// that budget.
///
/// Non-retryable errors and errors from `chat_completion_stream` itself (before
/// streaming starts) are propagated immediately.
///
/// [`Event::Notice`]s go to `notices` as they are consumed, and never appear in
/// the returned events.
/// A provider decision the user is owed — a credential skipped, a switch onto
/// per-token billing — happens whether or not the attempt that reported it
/// went on to succeed, so it is delivered before the outcome is known, and
/// reaches the user even when this returns an error.
pub async fn collect_with_retry(
    provider: &dyn Provider,
    model: &ModelDetails,
    query: ChatQuery,
    config: &RetryConfig,
    notices: &NoticeSink,
) -> Result<Vec<Event>> {
    let mut attempt = 0u32;

    loop {
        let stream = provider
            .chat_completion_stream(model, query.clone())
            .await?;

        // Bound a runaway response. These collect-style requests run with no
        // terminal attached (title generation, summarization, tool inquiries),
        // so nobody is watching to interrupt one that never stops.
        let stream = match config.max_response_bytes {
            Some(max) => with_output_limit(stream, max),
            None => stream,
        };

        let mut stream = std::pin::pin!(stream);
        let mut collected: Vec<Event> = vec![];
        let mut failure = None;

        while let Some(item) = stream.next().await {
            match item {
                Ok(Event::Notice(notice)) => notices.emit(&notice),
                Ok(event) => collected.push(event),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }

        let error = match failure {
            // Contract: a well-formed stream ends with `Event::Finished`. A
            // stream that ends without one was cut short (e.g. a dropped
            // connection); treat it as a transient failure and retry rather
            // than returning a truncated result.
            None if collected
                .last()
                .is_some_and(|e| matches!(e, Event::Finished(_))) =>
            {
                return Ok(collected);
            }
            None => StreamError::transient("provider stream ended without a terminal event"),
            Some(error) => error,
        };

        if error.kind == StreamErrorKind::CredentialChanged {
            continue;
        }
        attempt += 1;

        if !error.is_retryable() || attempt > config.max_retries {
            warn!(
                attempt,
                max = config.max_retries,
                error = error.to_string(),
                "Stream error (exhausted retries)."
            );
            return Err(error.into());
        }

        let delay = retry_delay(
            error.retry_after,
            attempt,
            config.base_backoff_ms,
            config.max_backoff_secs,
        );

        debug!(
            attempt,
            max = config.max_retries,
            delay_ms = delay.as_millis(),
            error = error.to_string(),
            "Retryable stream error, backing off."
        );

        tokio::time::sleep(delay).await;
    }
}

/// How long to wait before the next attempt, spread so that callers which
/// failed together do not resume together.
///
/// A `retry_after` the provider supplied is used as the delay, bounded by
/// `max_backoff_secs`.
/// Without one, the delay grows exponentially with `attempt`.
/// Either way a random extra of up to a quarter of that delay is added on top.
///
/// The extra is added after the bound rather than folded inside it, so it still
/// has an effect once the bound is reached, and it is only ever added, never
/// subtracted.
///
/// `max_backoff_secs` is authoritative over `retry_after`: a provider asking
/// for longer than the bound gets the bound, so the request may walk back into
/// the same limit.
#[must_use]
pub fn retry_delay(
    retry_after: Option<Duration>,
    attempt: u32,
    base_backoff_ms: u64,
    max_backoff_secs: u64,
) -> Duration {
    let base = match retry_after {
        Some(d) => d.min(Duration::from_secs(max_backoff_secs)),
        None => exponential_backoff(attempt, base_backoff_ms, max_backoff_secs),
    };

    // A quarter of a delay shorter than 4ms rounds to nothing, and asking for a
    // random value in an empty range panics.
    let window_ms = u64::try_from(base.as_millis() / 4).unwrap_or(u64::MAX);
    if window_ms == 0 {
        return base;
    }

    base + Duration::from_millis(rand::random_range(0..window_ms))
}

/// Calculate exponential backoff delay.
///
/// Formula: `min(base_backoff_ms * 2^(attempt - 1), max_backoff_secs * 1000)`
///
/// Callers wanting the delay to actually wait should use [`retry_delay`], which
/// adds the jitter that keeps concurrent callers apart.
///
/// # Arguments
///
/// - `attempt` - Current attempt number (1-based).
///   The delay doubles with each attempt.
/// - `base_backoff_ms` - Base delay in milliseconds for the first attempt.
/// - `max_backoff_secs` - Maximum delay cap in seconds.
#[must_use]
pub fn exponential_backoff(attempt: u32, base_backoff_ms: u64, max_backoff_secs: u64) -> Duration {
    let max_ms = max_backoff_secs * 1000;

    // Cap the exponent to avoid overflow.
    let capped_attempt = attempt.saturating_sub(1).min(20);
    let base_delay = base_backoff_ms.saturating_mul(1u64 << capped_attempt);
    let total_ms = base_delay.min(max_ms);

    Duration::from_millis(total_ms)
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
