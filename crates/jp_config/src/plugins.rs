//! Plugin configuration.
//!
//! Controls plugin installation, execution policy, and per-plugin options.
//!
//! See: `docs/rfd/D17-command-plugin-system.md`

pub mod command;

use indexmap::IndexMap;
use schematic::Config;

use crate::{
    FillDefaults,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    partial::ToPartial,
    plugins::command::CommandPluginConfig,
    util::merge_nested_indexmap,
};

/// Plugin configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct PluginsConfig {
    /// Whether to automatically install official plugins from the registry
    /// when they are first invoked.
    #[setting(default = true)]
    pub auto_install: bool,

    /// Grace period (in seconds) for plugin shutdown before force-killing.
    #[setting(default = 5)]
    pub shutdown_timeout_secs: u16,

    /// Command plugin configurations, keyed by plugin name (e.g. `serve`).
    #[setting(nested, merge = merge_nested_indexmap)]
    pub command: IndexMap<String, CommandPluginConfig>,
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
        use crate::delta::delta_opt;

        Self {
            auto_install: delta_opt(self.auto_install.as_ref(), next.auto_install),
            shutdown_timeout_secs: delta_opt(
                self.shutdown_timeout_secs.as_ref(),
                next.shutdown_timeout_secs,
            ),
            command: next
                .command
                .into_iter()
                .filter_map(|(name, next)| {
                    let next = match self.command.get(&name) {
                        Some(prev) if prev == &next => return None,
                        Some(prev) => prev.delta(next),
                        None => next,
                    };
                    Some((name, next))
                })
                .collect(),
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
            command: self.command,
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
            command: self
                .command
                .iter()
                .map(|(k, v)| (k.clone(), v.to_partial()))
                .collect(),
        }
    }
}
