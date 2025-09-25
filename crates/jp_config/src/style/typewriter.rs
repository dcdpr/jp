//! Typewriter effect styling configuration.

use std::time::Duration;

use schematic::{Config, Schematic};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::{delta_opt, PartialConfigDelta},
};

/// Typewriter style configuration.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct TypewriterConfig {
    /// Delay between printing characters.
    ///
    /// You can use one of the following formats:
    /// - `10` for 10 milliseconds
    /// - `5m` for 5 milliseconds
    /// - `1u` for 1 microsecond
    /// - `0` to disable
    #[config(default = "3")]
    pub text_delay: DelayDuration,

    /// Delay between printing characters.
    ///
    /// You can use one of the following formats:
    /// - `10` for 10 milliseconds
    /// - `5m` for 5 milliseconds
    /// - `1u` for 1 microsecond
    /// - `0` to disable
    #[config(default = "500u")]
    pub code_delay: DelayDuration,
}

impl AssignKeyValue for PartialTypewriterConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "text_delay" => self.text_delay = kv.try_some_from_str()?,
            "code_delay" => self.code_delay = kv.try_some_from_str()?,
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
        }
    }
}

/// Typewriter delay duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Schematic)]
pub struct DelayDuration(Duration);

impl std::str::FromStr for DelayDuration {
    type Err = String;

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

        num.map(Self)
            .ok_or_else(|| format!("invalid duration: {s}"))
    }
}

impl From<DelayDuration> for Duration {
    fn from(delay: DelayDuration) -> Self {
        delay.0
    }
}
