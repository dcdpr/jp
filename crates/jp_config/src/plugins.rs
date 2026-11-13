//! Plugin configuration.
//!
//! Controls plugin execution policy and per-plugin options.
//!
//! Installing is not configured: official plugins install the first time their
//! command is typed, and other plugins when you run `jp plugin install`.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

pub mod command;

use schematic::Config;
use serde_json::{Map, Value};

use crate::{
    FillDefaults,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key, moved_key},
    delta::{PartialConfigDelta, path},
    partial::ToPartial,
    plugins::command::{CommandPluginsConfig, PartialCommandPluginsConfig},
};

/// Plugin configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct PluginsConfig {
    /// Command plugins: standalone `jp-<name>` binaries that JP runs as `jp
    /// <name>`.
    ///
    /// Configure defaults under `[plugins.command.'*']` and individual plugins
    /// under `[plugins.command.<name>]`.
    #[setting(nested)]
    pub command: CommandPluginsConfig,
}

impl PluginsConfig {
    /// Return the configured `options` map for command plugin `name`, or an
    /// empty map if the plugin is not configured.
    #[must_use]
    pub fn command_options(&self, name: &str) -> Map<String, Value> {
        self.command
            .get(name)
            .map(|plugin| {
                plugin
                    .options
                    .iter()
                    .map(|(key, value)| (key.clone(), value.0.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl AssignKeyValue for PartialPluginsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "shutdown_timeout_secs" => {
                return moved_key(&kv, "plugins.command.*.shutdown_timeout_secs");
            }
            _ if kv.p("command") => self.command.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialPluginsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            command: self.command.delta(next.command),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            command: self
                .command
                .delta_with_unsets(next.command, &path(prefix, "command"), unsets),
        }
    }
}

impl FillDefaults for PartialPluginsConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            command: self.command.fill_from(defaults.command),
        }
    }
}

impl ToPartial for PluginsConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            command: self.command.to_partial(),
        }
    }
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
