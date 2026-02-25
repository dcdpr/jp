//! Retry utilities for resilient LLM request handling.

use std::time::Duration;

use futures::TryStreamExt as _;
use tracing::{debug, warn};

use crate::{Provider, error::Result, event::Event, model::ModelDetails, query::ChatQuery};

/// Configuration for resilient stream retries.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,

    /// Base backoff delay in milliseconds.
    pub base_backoff_ms: u64,

    /// Maximum backoff delay in seconds.
    pub max_backoff_secs: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_backoff_ms: 1000,
            max_backoff_secs: 30,
        }
    }
}

/// Execute `chat_completion_stream` with automatic retries on transient errors.
///
/// Collects the full event stream into a `Vec<Event>`. On retryable stream
/// errors, backs off and retries the entire request up to `config.max_retries`
/// times.
///
/// Non-retryable errors and errors from `chat_completion_stream` itself (before
/// streaming starts) are propagated immediately.
pub async fn collect_with_retry(
    provider: &dyn Provider,
    model: &ModelDetails,
    query: ChatQuery,
    config: &RetryConfig,
) -> Result<Vec<Event>> {
    let mut attempt = 0u32;

    loop {
        let stream = provider
            .chat_completion_stream(model, query.clone())
            .await?;

        match stream.try_collect::<Vec<Event>>().await {
            Ok(events) => return Ok(events),
            Err(error) => {
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

                let delay = match error.retry_after {
                    Some(d) => d.min(Duration::from_secs(config.max_backoff_secs)),
                    None => exponential_backoff(
                        attempt,
                        config.base_backoff_ms,
                        config.max_backoff_secs,
                    ),
                };

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
    }
}

/// Calculate exponential backoff delay.
///
/// Formula: `min(base * 2^attempt, max_backoff)`
///
/// # Arguments
///
/// * `attempt` - Current attempt number (1-based). The delay doubles with
///   each attempt.
/// * `base_backoff_ms` - Base delay in milliseconds for the first attempt.
/// * `max_backoff_secs` - Maximum delay cap in seconds.
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
