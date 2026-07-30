//! MCP server startup progress indicator configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
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
/// ```
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct McpStartupConfig {
    /// Whether to show the startup indicator.
    #[setting(default = true)]
    pub show: bool,

    /// Delay in seconds before showing the startup indicator.
    ///
    /// Servers that finish starting within this period never trigger the
    /// indicator.
    /// Set to 0 to show the indicator immediately.
    #[setting(default = 4)]
    pub delay_secs: u32,

    /// Interval in milliseconds between timer updates.
    #[setting(default = 100)]
    pub interval_ms: u32,
}

impl AssignKeyValue for PartialMcpStartupConfig {
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

impl PartialConfigDelta for PartialMcpStartupConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
            delay_secs: delta_opt(self.delay_secs.as_ref(), next.delay_secs),
            interval_ms: delta_opt(self.interval_ms.as_ref(), next.interval_ms),
        }
    }
}

impl FillDefaults for PartialMcpStartupConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            show: self.show.or(defaults.show),
            delay_secs: self.delay_secs.or(defaults.delay_secs),
            interval_ms: self.interval_ms.or(defaults.interval_ms),
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
        }
    }
}

#[cfg(test)]
#[path = "mcp_startup_tests.rs"]
mod tests;
