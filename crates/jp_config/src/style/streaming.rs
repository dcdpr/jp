//! Streaming response styling configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
};

/// Streaming response style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct StreamingConfig {
    /// Progress indicator configuration.
    ///
    /// Shows a waiting indicator while the LLM is processing the request.
    /// This covers the HTTP round-trip and time-to-first-token.
    #[setting(nested)]
    pub progress: ProgressConfig,
}

/// Progress indicator configuration for a wait with nothing to show but time.
///
/// Used by waits on something that produces no readable output of its own — an
/// HTTP response, a file lock — so there is no `print_stderr` key here.
///
/// ```toml
/// [style.streaming.progress]
/// delay_secs = 3
/// ```
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ProgressConfig {
    /// Whether to show the progress indicator.
    ///
    /// Defaults to `true`.
    #[setting(default = true)]
    pub show: bool,

    /// Delay in seconds before showing the progress indicator.
    ///
    /// Defaults to `3`.
    /// Waits shorter than this never show anything.
    /// Set to `0` to show it immediately.
    #[setting(default = 3)]
    pub delay_secs: u32,

    /// Interval in milliseconds between updates.
    ///
    /// Defaults to `100`.
    #[setting(default = 100)]
    pub interval_ms: u32,
}

impl AssignKeyValue for PartialProgressConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "show" => self.show = kv.try_some_bool()?,
            "delay_secs" => self.delay_secs = kv.try_some_u32()?,
            "interval_ms" => self.interval_ms = kv.try_some_u32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialProgressConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
            delay_secs: delta_opt(self.delay_secs.as_ref(), next.delay_secs),
            interval_ms: delta_opt(self.interval_ms.as_ref(), next.interval_ms),
        }
    }
}

impl FillDefaults for PartialProgressConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            show: self.show.or(defaults.show),
            delay_secs: self.delay_secs.or(defaults.delay_secs),
            interval_ms: self.interval_ms.or(defaults.interval_ms),
        }
    }
}

impl ToPartial for ProgressConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            show: partial_opt(&self.show, defaults.show),
            delay_secs: partial_opt(&self.delay_secs, defaults.delay_secs),
            interval_ms: partial_opt(&self.interval_ms, defaults.interval_ms),
        }
    }
}

impl AssignKeyValue for PartialStreamingConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("progress") => self.progress.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialStreamingConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            progress: self.progress.delta(next.progress),
        }
    }
}

impl FillDefaults for PartialStreamingConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            progress: self.progress.fill_from(defaults.progress),
        }
    }
}

impl ToPartial for StreamingConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            progress: self.progress.to_partial(),
        }
    }
}
