//! Typewriter effect styling configuration.

use std::time::Duration;

use schematic::{Config, Schematic};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
};

/// Typewriter style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TypewriterConfig {
    /// Delay between printing characters.
    ///
    /// The default is `3` milliseconds.
    ///
    /// You can use one of the following formats:
    /// - `10` for 10 milliseconds
    /// - `5m` for 5 milliseconds
    /// - `1u` for 1 microsecond
    /// - `0` to disable
    #[setting(default = "3")]
    pub text_delay: DelayDuration,

    /// Delay between printing code-block characters.
    ///
    /// The default is `500` microseconds.
    ///
    /// You can use one of the following formats:
    /// - `10` for 10 milliseconds
    /// - `5m` for 5 milliseconds
    /// - `1u` for 1 microsecond
    /// - `0` to disable
    #[setting(default = "500u")]
    pub code_delay: DelayDuration,

    /// Maximum latency the typewriter is allowed to fall behind the source.
    ///
    /// When set to a non-zero value, the typewriter acts as a bounded-
    /// latency controller: the effective per-character delay shrinks below
    /// `text_delay`/`code_delay` as the queue of pending characters grows,
    /// keeping printed output within `max_latency` of what the source has
    /// already emitted. With a fast provider (e.g. Cerebras) this prevents
    /// the typewriter from falling many seconds behind. When the source
    /// stops emitting, the controller switches to drain mode and stops
    /// slowing back down as the queue empties.
    ///
    /// The default is `0`, which disables the controller and keeps the
    /// static `text_delay`/`code_delay` behavior.
    ///
    /// Accepts the same formats as `text_delay`.
    #[setting(default = "0")]
    pub max_latency: DelayDuration,
}

impl AssignKeyValue for PartialTypewriterConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "text_delay" => self.text_delay = kv.try_some_from_str()?,
            "code_delay" => self.code_delay = kv.try_some_from_str()?,
            "max_latency" => self.max_latency = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialTypewriterConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            text_delay: delta_opt(self.text_delay.as_ref(), next.text_delay),
            code_delay: delta_opt(self.code_delay.as_ref(), next.code_delay),
            max_latency: delta_opt(self.max_latency.as_ref(), next.max_latency),
        }
    }
}

impl FillDefaults for PartialTypewriterConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            text_delay: self.text_delay.or(defaults.text_delay),
            code_delay: self.code_delay.or(defaults.code_delay),
            max_latency: self.max_latency.or(defaults.max_latency),
        }
    }
}

impl ToPartial for TypewriterConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            text_delay: partial_opt(&self.text_delay, defaults.text_delay),
            code_delay: partial_opt(&self.code_delay, defaults.code_delay),
            max_latency: partial_opt(&self.max_latency, defaults.max_latency),
        }
    }
}

/// Error when parsing `DelayDuration`.
#[derive(Debug, thiserror::Error)]
#[error("Invalid duration: {0}")]
pub struct DelayError(String);

/// Typewriter delay duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Schematic)]
pub struct DelayDuration(Duration);

impl DelayDuration {
    /// Sets the delay to `0`.
    #[must_use]
    pub const fn instant() -> Self {
        Self(Duration::from_secs(0))
    }
}

impl TryFrom<&str> for DelayDuration {
    type Error = DelayError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl std::str::FromStr for DelayDuration {
    type Err = DelayError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let num = s
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();

        let num = match s.get(num.len()..).unwrap_or_default() {
            "m" | "" => num.parse::<u64>().map(Duration::from_millis).ok(),
            "u" => num.parse::<u64>().map(Duration::from_micros).ok(),
            _ => None,
        };

        num.map(Self).ok_or_else(|| DelayError(s.to_owned()))
    }
}

impl From<DelayDuration> for Duration {
    fn from(delay: DelayDuration) -> Self {
        delay.0
    }
}
