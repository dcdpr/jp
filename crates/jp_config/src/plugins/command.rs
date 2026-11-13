//! Command plugin configuration.
//!
//! A command plugin is a standalone `jp-<name>` binary that JP runs as `jp
//! <name>`.
//! Configure defaults for all command plugins under `[plugins.command.'*']` and
//! individual plugins under `[plugins.command.<name>]`.
//!
//! ```toml
//! [plugins.command.'*']
//! shutdown_timeout_secs = 5
//!
//! [plugins.command.serve-web]
//! run = "allow"
//! ```

use std::time::Duration;

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{
        PartialConfigDelta, delta_mergeable_map, delta_mergeable_map_at, delta_mergeable_value_map,
        delta_mergeable_value_map_at, delta_opt, delta_opt_partial, path,
    },
    fill::{FillDefaults, fill_map},
    internal::merge::map_with_strategy,
    partial::{ToPartial, partial_opt, partial_opt_config, partial_opts},
    providers::mcp::{ChecksumConfig, PartialChecksumConfig},
    types::{
        json_value::JsonValue,
        map::{MergeableMap, map_to_partial_per_key},
    },
};

/// Command plugins configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case", allow_unknown_fields)]
pub struct CommandPluginsConfig {
    /// Defaults for every command plugin.
    ///
    /// Individual plugins can override these settings under
    /// `plugins.command.<name>`.
    #[setting(nested, rename = "*")]
    pub defaults: CommandPluginDefaultsConfig,

    /// Per-plugin configuration, keyed by plugin name (e.g. `serve-web`).
    ///
    /// Entries merge by key across configuration layers.
    /// Set `{ value = { ... }, strategy = "replace" }` to replace entries from
    /// earlier layers instead.
    #[setting(nested, flatten, merge = map_with_strategy)]
    plugins: MergeableMap<CommandPluginConfig>,
}

impl CommandPluginsConfig {
    /// Return the per-plugin configuration for `name`, if configured.
    ///
    /// Fields left unset on the returned [`CommandPluginConfig`] fall back to
    /// `self.defaults` (see [`Self::shutdown_timeout`]).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&CommandPluginConfig> {
        self.plugins.get(name)
    }

    /// Set the configuration for command plugin `name`, replacing any existing
    /// entry.
    pub fn insert(&mut self, name: String, plugin: CommandPluginConfig) {
        self.plugins.insert(name, plugin);
    }

    /// Return the graceful shutdown timeout for command plugin `name`.
    ///
    /// Uses the plugin's `shutdown_timeout_secs` if set, falling back to
    /// `defaults.shutdown_timeout_secs`.
    #[must_use]
    pub fn shutdown_timeout(&self, name: &str) -> Duration {
        let secs = self
            .plugins
            .get(name)
            .and_then(|plugin| plugin.shutdown_timeout_secs)
            .unwrap_or(self.defaults.shutdown_timeout_secs);

        Duration::from_secs(secs.into())
    }
}

impl AssignKeyValue for PartialCommandPluginsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            // Because `plugins` is flattened, a `{ value, strategy }` map
            // wrapper arrives at the empty key alongside plain per-plugin
            // objects. Route it directly to `self.plugins` so `value` and
            // `strategy` are not treated as plugin names.
            "" if kv.states_map_strategy() => kv.assign_to_mergeable_entry(&mut self.plugins)?,
            "" => kv.try_merge_object(self)?,
            _ if kv.p("*") => self.defaults.assign(kv)?,
            _ => kv.assign_to_mergeable_entry(&mut self.plugins)?,
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCommandPluginsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            defaults: self.defaults.delta(next.defaults),
            plugins: delta_mergeable_map(&self.plugins, next.plugins),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            defaults: self
                .defaults
                .delta_with_unsets(next.defaults, &path(prefix, "*"), unsets),
            plugins: delta_mergeable_map_at(prefix, &self.plugins, next.plugins, unsets),
        }
    }
}

impl FillDefaults for PartialCommandPluginsConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            defaults: self.defaults.fill_from(defaults.defaults),
            // Fill missing plugin entries from `defaults` without overwriting
            // entries or an explicit merge strategy set on `self`.
            plugins: match self.plugins {
                merged @ MergeableMap::Merged(_) => merged,
                MergeableMap::Map(entries) => fill_map(entries, defaults.plugins.into_map()).into(),
            },
        }
    }
}

impl ToPartial for CommandPluginsConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            defaults: self.defaults.to_partial(),
            // Emit per-key entries rather than a `replace` map so plugins added
            // in lower layers remain visible.
            plugins: map_to_partial_per_key(self.plugins.iter()),
        }
    }
}

/// Defaults for every command plugin.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct CommandPluginDefaultsConfig {
    /// Whether command plugins may run.
    ///
    /// Defaults to `ask`.
    ///
    /// - `ask`: run official plugin releases and previously approved binaries
    ///   without prompting; prompt before running any other binary.
    /// - `allow`: run without prompting.
    /// - `deny`: never run.
    ///
    /// `unattended` is accepted as a deprecated alias for `allow`.
    ///
    /// Applies to every command plugin that does not set its own
    /// `plugins.command.<name>.run`.
    /// Set this to `deny` to run only the plugins you allow by name:
    ///
    /// ```toml
    /// [plugins.command.'*']
    /// run = "deny"
    ///
    /// [plugins.command.serve-web]
    /// run = "allow"
    /// ```
    ///
    /// Setting this to `allow` runs every `jp-*` binary on `$PATH` without
    /// prompting.
    #[setting(default)]
    pub run: RunPolicy,

    /// Grace period in seconds for a command plugin to exit after JP sends a
    /// `shutdown` request.
    ///
    /// Defaults to `5`.
    /// When the timeout expires, JP terminates the plugin and its process tree.
    /// Set to `0` to kill plugins immediately when shutdown is requested.
    ///
    /// Applies to every command plugin that does not set its own
    /// `plugins.command.<name>.shutdown_timeout_secs`.
    #[setting(default = 5)]
    pub shutdown_timeout_secs: u16,
}

impl AssignKeyValue for PartialCommandPluginDefaultsConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "run" => self.run = kv.try_some_from_str()?,
            "shutdown_timeout_secs" => {
                self.shutdown_timeout_secs = kv.try_some_number_or_from_str()?;
            }
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCommandPluginDefaultsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            run: delta_opt(self.run.as_ref(), next.run),
            shutdown_timeout_secs: delta_opt(
                self.shutdown_timeout_secs.as_ref(),
                next.shutdown_timeout_secs,
            ),
        }
    }
}

impl FillDefaults for PartialCommandPluginDefaultsConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            run: self.run.or(defaults.run),
            shutdown_timeout_secs: self
                .shutdown_timeout_secs
                .or(defaults.shutdown_timeout_secs),
        }
    }
}

impl ToPartial for CommandPluginDefaultsConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            run: partial_opt(&self.run, defaults.run),
            shutdown_timeout_secs: partial_opt(
                &self.shutdown_timeout_secs,
                defaults.shutdown_timeout_secs,
            ),
        }
    }
}

/// Execution policy for a command plugin.
#[derive(Debug, Clone, Copy, PartialEq, Default, schematic::ConfigEnum)]
#[config(rename_all = "snake_case", serde_as_string)]
pub enum RunPolicy {
    /// Run official plugin releases and previously approved binaries without
    /// prompting; prompt before running any other binary.
    #[default]
    Ask,
    /// Run without prompting.
    #[variant(deprecated_aliases("unattended"))]
    Allow,
    /// Never run this plugin.
    Deny,
}

/// Configuration for a single command plugin.
///
/// Example:
///
/// ```toml
/// [plugins.command.serve-web]
/// run = "allow"
///
/// [plugins.command.serve-web.checksum]
/// algorithm = "sha256"
/// value = "abc123..."
///
/// [plugins.command.serve-web.options]
/// bind = "0.0.0.0"
/// port = 2000
/// ```
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct CommandPluginConfig {
    /// Execution policy for this plugin.
    ///
    /// If unset, falls back to `plugins.command.'*'.run`.
    ///
    /// - `ask`: run official plugin releases and previously approved binaries
    ///   without prompting; prompt before running any other binary.
    /// - `allow`: run without prompting.
    /// - `deny`: never run this plugin.
    ///
    /// `unattended` is accepted as a deprecated alias for `allow`.
    pub run: Option<RunPolicy>,

    /// Pinned binary checksum.
    ///
    /// When set, JP refuses to run the plugin if the binary's checksum doesn't
    /// match.
    /// This protects against unexpected binary changes.
    #[setting(nested)]
    pub checksum: Option<ChecksumConfig>,

    /// Grace period in seconds for this plugin to exit after JP sends a
    /// `shutdown` request.
    ///
    /// If unset, falls back to `plugins.command.'*'.shutdown_timeout_secs`.
    /// When the timeout expires, JP terminates the plugin and its process tree.
    /// Set to `0` to kill the plugin immediately when shutdown is requested.
    ///
    /// Increase this for plugins that need extra time to finish in-flight work,
    /// such as waiting for an active turn to complete:
    ///
    /// ```toml
    /// [plugins.command.serve-chat]
    /// shutdown_timeout_secs = 90
    /// ```
    pub shutdown_timeout_secs: Option<u16>,

    /// Opaque options passed to the plugin in the `init` message.
    ///
    /// JP does not validate these; they are forwarded as-is in the `config`
    /// field of the `init` message.
    /// The plugin is responsible for parsing and error reporting.
    ///
    /// Options merge key by key across configuration layers, recursing into
    /// nested tables.
    /// Set `{ value = { ... }, strategy = "replace" }` to replace options from
    /// earlier layers instead.
    #[setting(nested, merge = map_with_strategy)]
    pub options: MergeableMap<JsonValue>,
}

impl AssignKeyValue for PartialCommandPluginConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "run" => self.run = kv.try_some_from_str()?,
            "shutdown_timeout_secs" => {
                self.shutdown_timeout_secs = kv.try_some_number_or_from_str()?;
            }
            _ if kv.p("checksum") => self.checksum.assign(kv)?,
            _ if kv.p("options") => kv.assign_to_mergeable_entry(&mut self.options)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCommandPluginConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            run: delta_opt(self.run.as_ref(), next.run),
            checksum: delta_opt_partial(self.checksum.as_ref(), next.checksum),
            shutdown_timeout_secs: delta_opt(
                self.shutdown_timeout_secs.as_ref(),
                next.shutdown_timeout_secs,
            ),
            options: delta_mergeable_value_map(&self.options, next.options),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            run: delta_opt(self.run.as_ref(), next.run),
            checksum: delta_opt_partial(self.checksum.as_ref(), next.checksum),
            shutdown_timeout_secs: delta_opt(
                self.shutdown_timeout_secs.as_ref(),
                next.shutdown_timeout_secs,
            ),
            options: delta_mergeable_value_map_at(
                &path(prefix, "options"),
                &self.options,
                next.options,
                unsets,
            ),
        }
    }
}

impl ToPartial for CommandPluginConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            run: partial_opts(self.run.as_ref(), defaults.run),
            checksum: partial_opt_config(self.checksum.as_ref(), defaults.checksum),
            shutdown_timeout_secs: partial_opts(
                self.shutdown_timeout_secs.as_ref(),
                defaults.shutdown_timeout_secs,
            ),
            options: MergeableMap::Map(
                self.options
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
