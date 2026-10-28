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
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_mergeable_map, delta_mergeable_map_at, delta_opt, path},
    fill::fill_map,
    internal::merge::map_with_strategy,
    partial::ToPartial,
    plugins::command::CommandPluginConfig,
    types::map::{MergeableMap, map_to_partial_per_key},
};

/// Plugin configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct PluginsConfig {
    /// Grace period (in seconds) for plugin shutdown before force-killing.
    #[setting(default = 5)]
    pub shutdown_timeout_secs: u16,

    /// Command plugin configurations, keyed by plugin name (e.g. `serve`).
    ///
    /// Entries merge by key, so a plugin configured in a later layer joins the
    /// ones an earlier layer set.
    /// Declare the map as `{ value = { … }, strategy = "replace" }` to drop
    /// them instead.
    #[setting(nested, merge = map_with_strategy)]
    pub command: MergeableMap<CommandPluginConfig>,
}

impl PluginsConfig {
    /// The `options` the command plugin `name` is configured with, empty when
    /// it has none.
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
            "shutdown_timeout_secs" => self.shutdown_timeout_secs = kv.try_some_from_str()?,
            _ if kv.p("command") => kv.assign_to_mergeable_entry(&mut self.command)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialPluginsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            shutdown_timeout_secs: delta_opt(
                self.shutdown_timeout_secs.as_ref(),
                next.shutdown_timeout_secs,
            ),
            command: delta_mergeable_map(&self.command, next.command),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            shutdown_timeout_secs: delta_opt(
                self.shutdown_timeout_secs.as_ref(),
                next.shutdown_timeout_secs,
            ),
            command: delta_mergeable_map_at(
                &path(prefix, "command"),
                &self.command,
                next.command,
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialPluginsConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            shutdown_timeout_secs: self
                .shutdown_timeout_secs
                .or(defaults.shutdown_timeout_secs),
            // Key by key, so a plugin only the defaults declare is added
            // while one this layer already has keeps its own value. A map
            // that states a strategy is left alone.
            command: match self.command {
                merged @ MergeableMap::Merged(_) => merged,
                MergeableMap::Map(entries) => fill_map(entries, defaults.command.into_map()).into(),
            },
        }
    }
}

impl ToPartial for PluginsConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            shutdown_timeout_secs: crate::partial::partial_opt(
                &self.shutdown_timeout_secs,
                defaults.shutdown_timeout_secs,
            ),
            // Per key rather than `replace`: a plugin the workspace config
            // gained after this conversation was created still reaches it.
            command: map_to_partial_per_key(self.command.iter()),
        }
    }
}
