//! Command plugin configuration.
//!
//! Per-plugin settings that control installation, execution policy,
//! checksum pinning, and opaque options passed through to the plugin.

use schematic::Config;
use serde_json::Value;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial},
    partial::{ToPartial, partial_opt_config, partial_opts},
    providers::mcp::{ChecksumConfig, PartialChecksumConfig},
};

/// Execution policy for a command plugin.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    schematic::ConfigEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum RunPolicy {
    /// Prompt the user before running (default for third-party plugins).
    #[default]
    Ask,
    /// Run without prompting (default for official registry plugins).
    Unattended,
    /// Never run this plugin.
    Deny,
}

/// Configuration for a single command plugin.
///
/// Example:
///
/// ```toml
/// [plugins.command.serve]
/// install = true
/// run = "unattended"
///
/// [plugins.command.serve.checksum]
/// algorithm = "sha256"
/// value = "abc123..."
///
/// [plugins.command.serve.options]
/// web.port = 2000
/// web.host = "0.0.0.0"
/// ```
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct CommandPluginConfig {
    /// Whether to auto-install this plugin from the registry if it is missing.
    ///
    /// When `true`, JP will download and install the plugin binary from the
    /// registry on first invocation. Defaults to the global
    /// `plugins.auto_install` setting.
    pub install: Option<bool>,

    /// Execution policy.
    ///
    /// - `ask`: prompt before running (default for third-party plugins)
    /// - `unattended`: run without prompting
    /// - `deny`: never run this plugin
    pub run: Option<RunPolicy>,

    /// Pinned binary checksum.
    ///
    /// When set, JP refuses to run the plugin if the binary's checksum
    /// doesn't match. This protects against unexpected binary changes.
    /// Uses the same `ChecksumConfig` as MCP servers.
    #[setting(nested)]
    pub checksum: Option<ChecksumConfig>,

    /// Opaque options passed to the plugin in the `init` message.
    ///
    /// JP does not validate these — they are forwarded as-is in the config
    /// section of the init message. The plugin is responsible for parsing
    /// and error reporting.
    pub options: Option<Value>,
}

impl AssignKeyValue for PartialCommandPluginConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "install" => self.install = kv.try_some_bool()?,
            "run" => self.run = kv.try_some_from_str()?,
            _ if kv.p("checksum") => self.checksum.assign(kv)?,
            _ if kv.p("options") => {
                self.options = Some(kv.value.into_value());
            }
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCommandPluginConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            install: delta_opt(self.install.as_ref(), next.install),
            run: delta_opt(self.run.as_ref(), next.run),
            checksum: delta_opt_partial(self.checksum.as_ref(), next.checksum),
            options: delta_opt(self.options.as_ref(), next.options),
        }
    }
}

impl ToPartial for CommandPluginConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            install: partial_opts(self.install.as_ref(), defaults.install),
            run: partial_opts(self.run.as_ref(), defaults.run),
            checksum: partial_opt_config(self.checksum.as_ref(), defaults.checksum),
            options: partial_opts(self.options.as_ref(), defaults.options),
        }
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
