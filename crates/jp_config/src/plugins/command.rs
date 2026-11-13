//! Command plugin configuration.
//!
//! A command plugin is a standalone `jp-<name>` binary that JP runs as `jp
//! <name>`.
//! `[plugins.command.'*']` sets defaults for all of them, and
//! `[plugins.command.<name>]` configures one: its execution policy, a pinned
//! checksum, how long it gets to shut down, and the options passed to it.
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
    /// A plugin that sets the same key under `plugins.command.<name>` uses its
    /// own value instead.
    #[setting(nested, rename = "*")]
    pub defaults: CommandPluginDefaultsConfig,

    /// Per-plugin configuration, keyed by plugin name (e.g. `serve-web`).
    ///
    /// Entries merge by key, so a plugin configured in a later layer joins the
    /// ones an earlier layer set.
    /// Declare the map as `{ value = { … }, strategy = "replace" }` to drop
    /// them instead.
    #[setting(nested, flatten, merge = map_with_strategy)]
    plugins: MergeableMap<CommandPluginConfig>,
}

impl CommandPluginsConfig {
    /// The configuration of the command plugin `name`, if it has any.
    ///
    /// Holds what the plugin sets for itself.
    /// A value it leaves to the defaults is read through the accessor for that
    /// value, such as [`Self::shutdown_timeout`].
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&CommandPluginConfig> {
        self.plugins.get(name)
    }

    /// Configure the command plugin `name`, replacing what it was configured
    /// with.
    pub fn insert(&mut self, name: String, plugin: CommandPluginConfig) {
        self.plugins.insert(name, plugin);
    }

    /// How long the command plugin `name` gets to exit once it is asked to shut
    /// down.
    ///
    /// The plugin's own `shutdown_timeout_secs`, or the default for a plugin
    /// that sets none or is not configured at all.
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
            // The plugins map is flattened, so a wrapper stating how it merges
            // arrives with no key of its own, where an object naming several
            // plugins also arrives. Its `value` and `strategy` are the
            // wrapper's keys, not plugin names, so it goes to the map rather
            // than being dispatched key by key.
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
            // Key by key, so a plugin only the defaults declare is added
            // while one this layer already has keeps its own value. A map
            // that states a strategy is left alone.
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
            // Per key rather than `replace`: a plugin the workspace config
            // gained after this conversation was created still reaches it.
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
    /// - `ask`: run an official plugin's published release and a binary you
    ///   approved, and prompt before running any other.
    /// - `allow`: run without prompting.
    /// - `deny`: never run.
    ///
    /// `unattended` is accepted as a deprecated spelling of `allow`.
    ///
    /// Applies to every command plugin that does not set its own
    /// `plugins.command.<name>.run`.
    /// Set it to `deny` to run only the plugins you allow by name:
    ///
    /// ```toml
    /// [plugins.command.'*']
    /// run = "deny"
    ///
    /// [plugins.command.serve-web]
    /// run = "allow"
    /// ```
    ///
    /// Set to `allow`, it runs every `jp-*` binary found on `$PATH` without
    /// asking.
    #[setting(default)]
    pub run: RunPolicy,

    /// How long a command plugin gets to exit after JP asks it to shut down, in
    /// seconds.
    ///
    /// Defaults to `5`.
    /// Once the time is up, JP kills the plugin and every process it started.
    /// Set to `0` to kill plugins as soon as shutdown is requested.
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
    /// Run an official plugin's published release and an approved binary, and
    /// prompt the user before running any other.
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
    /// Execution policy.
    ///
    /// If unset, falls back to `plugins.command.'*'.run`.
    ///
    /// - `ask`: run the binary if it is the official plugin's published release
    ///   or one you approved, and prompt before running it otherwise
    /// - `allow`: run without prompting
    /// - `deny`: never run this plugin
    ///
    /// `unattended` is accepted as a deprecated spelling of `allow`.
    pub run: Option<RunPolicy>,

    /// Pinned binary checksum.
    ///
    /// When set, JP refuses to run the plugin if the binary's checksum doesn't
    /// match.
    /// This protects against unexpected binary changes.
    /// A checksum is one binary's, so `plugins.command.'*'` has no default for
    /// it.
    /// Uses the same `ChecksumConfig` as MCP servers.
    #[setting(nested)]
    pub checksum: Option<ChecksumConfig>,

    /// How long this plugin gets to exit after JP asks it to shut down, in
    /// seconds.
    ///
    /// If unset, falls back to `plugins.command.'*'.shutdown_timeout_secs`.
    /// Once the time is up, JP kills the plugin and every process it started.
    /// Set to `0` to kill the plugin as soon as shutdown is requested.
    ///
    /// Raise it for a plugin that has work to finish before it exits, such as
    /// one that waits for a running turn to stop:
    ///
    /// ```toml
    /// [plugins.command.serve-chat]
    /// shutdown_timeout_secs = 90
    /// ```
    pub shutdown_timeout_secs: Option<u16>,

    /// Opaque options passed to the plugin in the `init` message.
    ///
    /// JP does not validate these — they are forwarded as-is in the config
    /// section of the init message.
    /// The plugin is responsible for parsing and error reporting.
    ///
    /// Options merge key by key, recursing into nested tables, so a later layer
    /// only replaces the options it names.
    /// Declare the map as `{ value = { … }, strategy = "replace" }` to drop
    /// the options earlier layers set instead.
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
