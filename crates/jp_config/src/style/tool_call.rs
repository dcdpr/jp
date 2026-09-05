//! Tool call styling configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
    style::print_stderr::PrintStderr,
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
    pub progress: ToolProgressConfig,

    /// Preparing indicator configuration.
    ///
    /// Controls the "(receiving arguments… Ns)" suffix shown after the
    /// "Calling tool X" header while arguments are still streaming.
    ///
    /// Note: the "Calling tool X" header itself is always shown immediately
    /// when the tool name is known.
    /// This config only controls the animated suffix that indicates arguments
    /// are still being received.
    #[setting(nested)]
    pub preparing: PreparingConfig,
}

/// Progress indicator configuration for tool execution.
///
/// ```toml
/// [style.tool_call.progress]
/// delay_secs = 3
/// print_stderr = false
/// ```
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolProgressConfig {
    /// Whether to show the progress indicator.
    ///
    /// Defaults to `true`.
    #[setting(default = true)]
    pub show: bool,

    /// Delay in seconds before showing the progress indicator.
    ///
    /// Defaults to `3`.
    /// Tools that finish faster than this never show anything.
    /// Set to `0` to show progress immediately.
    #[setting(default = 3)]
    pub delay_secs: u32,

    /// Interval in milliseconds between progress updates.
    ///
    /// Defaults to `100`.
    #[setting(default = 100)]
    pub interval_ms: u32,

    /// Rows of the tool's stderr to show above the timer.
    ///
    /// - `false` or `0`: show the timer alone.
    ///   This is the default.
    /// - `true`: size the window from the terminal height.
    /// - `N`: show exactly `N` rows.
    ///
    /// Defaults to `false`, unlike `style.mcp_startup.print_stderr`: a tool's
    /// progress can be on screen while the assistant's answer is streaming
    /// beneath it, where extra rows cost the most.
    /// The lines are erased with the timer and never reach the transcript; the
    /// tool's full output still goes to the assistant either way.
    #[setting(default = "off")]
    pub print_stderr: PrintStderr,
}

/// Configuration for the "(receiving arguments…)" indicator shown while tool
/// call arguments are still streaming from the LLM.
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
    /// The "Calling tool X" header is always shown immediately.
    /// This delay controls when the animated "(receiving arguments… Ns)"
    /// suffix appears.
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
            "" => kv.try_merge_object(self)?,
            "show" => self.show = kv.try_some_bool()?,
            _ if kv.p("progress") => self.progress.assign(kv)?,
            _ if kv.p("preparing") => self.preparing.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialToolProgressConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "show" => self.show = kv.try_some_bool()?,
            "delay_secs" => self.delay_secs = kv.try_some_u32()?,
            "interval_ms" => self.interval_ms = kv.try_some_u32()?,
            "print_stderr" => self.print_stderr = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialPreparingConfig {
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

impl PartialConfigDelta for PartialToolCallConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
            progress: self.progress.delta(next.progress),
            preparing: self.preparing.delta(next.preparing),
        }
    }
}

impl PartialConfigDelta for PartialToolProgressConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
            delay_secs: delta_opt(self.delay_secs.as_ref(), next.delay_secs),
            interval_ms: delta_opt(self.interval_ms.as_ref(), next.interval_ms),
            print_stderr: delta_opt(self.print_stderr.as_ref(), next.print_stderr),
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

impl FillDefaults for PartialToolCallConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            show: self.show.or(defaults.show),
            progress: self.progress.fill_from(defaults.progress),
            preparing: self.preparing.fill_from(defaults.preparing),
        }
    }
}

impl FillDefaults for PartialToolProgressConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            show: self.show.or(defaults.show),
            delay_secs: self.delay_secs.or(defaults.delay_secs),
            interval_ms: self.interval_ms.or(defaults.interval_ms),
            print_stderr: self.print_stderr.or(defaults.print_stderr),
        }
    }
}

impl FillDefaults for PartialPreparingConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            show: self.show.or(defaults.show),
            delay_secs: self.delay_secs.or(defaults.delay_secs),
            interval_ms: self.interval_ms.or(defaults.interval_ms),
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

impl ToPartial for ToolProgressConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            show: partial_opt(&self.show, defaults.show),
            delay_secs: partial_opt(&self.delay_secs, defaults.delay_secs),
            interval_ms: partial_opt(&self.interval_ms, defaults.interval_ms),
            print_stderr: partial_opt(&self.print_stderr, defaults.print_stderr),
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
