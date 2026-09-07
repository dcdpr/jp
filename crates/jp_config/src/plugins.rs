//! Plugin configuration.
//!
//! Controls plugin installation, execution policy, and per-plugin options.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

pub mod command;

use schematic::Config;

use crate::{
    FillDefaults,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    fill::fill_map,
    delta::{PartialConfigDelta, delta_mergeable_map, delta_opt},
    internal::merge::map_with_strategy,
    partial::ToPartial,
    plugins::command::CommandPluginConfig,
    types::map::{MergeableMap, map_to_partial_per_key},
};

/// Plugin configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct PluginsConfig {
    /// Whether to automatically install official plugins from the registry when
    /// they are first invoked.
    #[setting(default = true)]
    pub auto_install: bool,

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

impl AssignKeyValue for PartialPluginsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "auto_install" => self.auto_install = kv.try_some_bool()?,
            "shutdown_timeout_secs" => self.shutdown_timeout_secs = kv.try_some_from_str()?,
            _ if kv.p("command") => match kv.trim_prefix_any() {
                Some(name) => self.command.entry(name).or_default().assign(kv)?,
                None => return missing_key(&kv),
            },
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialPluginsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auto_install: delta_opt(self.auto_install.as_ref(), next.auto_install),
            shutdown_timeout_secs: delta_opt(
                self.shutdown_timeout_secs.as_ref(),
                next.shutdown_timeout_secs,
            ),
            command: delta_mergeable_map(&self.command, next.command),
        }
    }
}

impl FillDefaults for PartialPluginsConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            auto_install: self.auto_install.or(defaults.auto_install),
            shutdown_timeout_secs: self
                .shutdown_timeout_secs
                .or(defaults.shutdown_timeout_secs),
            // Key by key, so a plugin only the defaults declare is added
            // while one this layer already has keeps its own value. A map
            // that states a strategy is left alone.
            command: match self.command {
                merged @ MergeableMap::Merged(_) => merged,
                MergeableMap::Map(entries) => {
                    fill_map(entries, defaults.command.into_map()).into()
                }
            },
        }
    }
}

impl ToPartial for PluginsConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            auto_install: crate::partial::partial_opt(&self.auto_install, defaults.auto_install),
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
