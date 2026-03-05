//! LLM request behavior configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
};

/// Configuration for LLM request behavior.
///
/// Controls retry logic for transient errors like rate limits, timeouts, and
/// connection failures.
#[derive(Debug, Clone, Copy, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct RequestConfig {
    /// Maximum retry attempts for transient errors.
    ///
    /// Retryable errors include rate limits, timeouts, connection errors, and
    /// transient server errors (5xx). Set to 0 to disable retries.
    ///
    /// Non-retryable errors (auth failures, unknown models, invalid requests)
    /// are never retried regardless of this setting.
    #[setting(default = 5)]
    pub max_retries: u32,

    /// Base delay for exponential backoff (in milliseconds).
    ///
    /// The actual delay is calculated as:
    ///
    /// ```text
    /// delay = min(base_backoff * 2^attempt + jitter, max_backoff)
    /// ```
    ///
    /// Where jitter is a random value between 0-500ms to prevent thundering
    /// herd problems.
    #[setting(default = 1000)]
    pub base_backoff_ms: u32,

    /// Maximum backoff delay (in seconds).
    ///
    /// The backoff delay will never exceed this value, regardless of the number
    /// of retry attempts.
    #[setting(default = 60)]
    pub max_backoff_secs: u32,
}

impl AssignKeyValue for PartialRequestConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "max_retries" => self.max_retries = kv.try_some_u32()?,
            "base_backoff_ms" => self.base_backoff_ms = kv.try_some_u32()?,
            "max_backoff_secs" => self.max_backoff_secs = kv.try_some_u32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialRequestConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            max_retries: delta_opt(self.max_retries.as_ref(), next.max_retries),
            base_backoff_ms: delta_opt(self.base_backoff_ms.as_ref(), next.base_backoff_ms),
            max_backoff_secs: delta_opt(self.max_backoff_secs.as_ref(), next.max_backoff_secs),
        }
    }
}

impl ToPartial for RequestConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            max_retries: partial_opt(&self.max_retries, defaults.max_retries),
            base_backoff_ms: partial_opt(&self.base_backoff_ms, defaults.base_backoff_ms),
            max_backoff_secs: partial_opt(&self.max_backoff_secs, defaults.max_backoff_secs),
        }
    }
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
