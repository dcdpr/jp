//! Tool call styling configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
};

/// Tool call content style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolCallConfig {
    /// Whether to show the "tool call" text.
    ///
    /// Even if this is disabled, the model can still call tools and receive the
    /// results, but it will not be displayed.
    #[setting(default = true)]
    pub show: bool,

    /// Progress indicator configuration.
    ///
    /// Shows elapsed time for long-running tool executions.
    #[setting(nested)]
    pub progress: ProgressConfig,

    /// Preparing indicator configuration.
    ///
    /// Controls the "(receiving arguments… Ns)" suffix shown after the
    /// "Calling tool X" header while arguments are still streaming.
    ///
    /// Note: the "Calling tool X" header itself is always shown immediately
    /// when the tool name is known. This config only controls the animated
    /// suffix that indicates arguments are still being received.
    #[setting(nested)]
    pub preparing: PreparingConfig,
}

/// Progress indicator configuration for tool execution.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ProgressConfig {
    /// Whether to show the progress indicator.
    #[setting(default = true)]
    pub show: bool,

    /// Delay in seconds before showing progress indicator.
    ///
    /// Progress is only shown for tools that run longer than this threshold.
    /// Set to 0 to show progress immediately.
    #[setting(default = 3)]
    pub delay_secs: u32,

    /// Interval in milliseconds between progress updates.
    #[setting(default = 100)]
    pub interval_ms: u32,
}

/// Configuration for the "(receiving arguments…)" indicator shown while
/// tool call arguments are still streaming from the LLM.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct PreparingConfig {
    /// Whether to show the "(receiving arguments…)" suffix.
    ///
    /// When disabled, only the "Calling tool X" header is shown immediately,
    /// with no animated suffix while arguments stream.
    #[setting(default = true)]
    pub show: bool,

    /// Delay in seconds before appending the "(receiving arguments…)" suffix.
    ///
    /// The "Calling tool X" header is always shown immediately. This delay
    /// controls when the animated "(receiving arguments… Ns)" suffix appears.
    /// Set to 0 to show the suffix immediately.
    #[setting(default = 3)]
    pub delay_secs: u32,

    /// Interval in milliseconds between timer updates in the suffix.
    #[setting(default = 100)]
    pub interval_ms: u32,
}

impl AssignKeyValue for PartialToolCallConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "show" => self.show = kv.try_some_bool()?,
            _ if kv.p("progress") => self.progress.assign(kv)?,
            _ if kv.p("preparing") => self.preparing.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialProgressConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "show" => self.show = kv.try_some_bool()?,
            "delay_secs" => self.delay_secs = kv.try_some_u32()?,
            "interval_ms" => self.interval_ms = kv.try_some_u32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialPreparingConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "show" => self.show = kv.try_some_bool()?,
            "delay_secs" => self.delay_secs = kv.try_some_u32()?,
            "interval_ms" => self.interval_ms = kv.try_some_u32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialToolCallConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
            progress: self.progress.delta(next.progress),
            preparing: self.preparing.delta(next.preparing),
        }
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

impl PartialConfigDelta for PartialPreparingConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
            delay_secs: delta_opt(self.delay_secs.as_ref(), next.delay_secs),
            interval_ms: delta_opt(self.interval_ms.as_ref(), next.interval_ms),
        }
    }
}

impl ToPartial for ToolCallConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            show: partial_opt(&self.show, defaults.show),
            progress: self.progress.to_partial(),
            preparing: self.preparing.to_partial(),
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

impl ToPartial for PreparingConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            show: partial_opt(&self.show, defaults.show),
            delay_secs: partial_opt(&self.delay_secs, defaults.delay_secs),
            interval_ms: partial_opt(&self.interval_ms, defaults.interval_ms),
        }
    }
}
