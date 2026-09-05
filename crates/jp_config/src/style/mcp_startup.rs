//! MCP server startup progress indicator configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
    style::print_stderr::PrintStderr,
};

/// Progress indicator shown while MCP servers are starting.
///
/// Enabled MCP servers boot in the background when a query starts.
/// When one or more of them are still starting by the time the query needs
/// them, the CLI waits and shows a timer listing the pending servers.
///
/// ```toml
/// [style.mcp_startup]
/// delay_secs = 4
/// print_stderr = true
/// ```
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct McpStartupConfig {
    /// Whether to show the startup indicator.
    ///
    /// Defaults to `true`.
    #[setting(default = true)]
    pub show: bool,

    /// Delay in seconds before showing the startup indicator.
    ///
    /// Defaults to `4`.
    /// Servers that finish starting within this period never trigger the
    /// indicator.
    /// Set to 0 to show the indicator immediately.
    #[setting(default = 4)]
    pub delay_secs: u32,

    /// Interval in milliseconds between timer updates.
    ///
    /// Defaults to `100`.
    #[setting(default = 100)]
    pub interval_ms: u32,

    /// Rows of the starting servers' stderr to show above the timer.
    ///
    /// - `false` or `0`: show the timer alone.
    /// - `true`: size the window from the terminal height.
    ///   This is the default.
    /// - `N`: show exactly `N` rows.
    ///
    /// A server that builds from source on first launch can take minutes, and a
    /// bare climbing number cannot be told apart from a hang; the compiler's
    /// own progress can.
    /// The lines are erased with the timer and never reach the transcript.
    ///
    /// Set to `false` if a startup wrapper echoes anything you would rather not
    /// have on screen during a screen share — a resolved token, say.
    /// The rows are the child's own output and nothing inspects them for
    /// secrets.
    #[setting(default = "auto")]
    pub print_stderr: PrintStderr,
}

impl AssignKeyValue for PartialMcpStartupConfig {
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

impl PartialConfigDelta for PartialMcpStartupConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
            delay_secs: delta_opt(self.delay_secs.as_ref(), next.delay_secs),
            interval_ms: delta_opt(self.interval_ms.as_ref(), next.interval_ms),
            print_stderr: delta_opt(self.print_stderr.as_ref(), next.print_stderr),
        }
    }
}

impl FillDefaults for PartialMcpStartupConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            show: self.show.or(defaults.show),
            delay_secs: self.delay_secs.or(defaults.delay_secs),
            interval_ms: self.interval_ms.or(defaults.interval_ms),
            print_stderr: self.print_stderr.or(defaults.print_stderr),
        }
    }
}

impl ToPartial for McpStartupConfig {
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

#[cfg(test)]
#[path = "mcp_startup_tests.rs"]
mod tests;
