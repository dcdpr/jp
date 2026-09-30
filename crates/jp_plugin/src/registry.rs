//! Plugin registry types.
//!
//! The registry is a JSON file served from the JP registry server that lists
//! available plugins with their platform-specific download URLs and checksums.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::Manifest;

/// The plugin registry, fetched from the JP registry server.
///
/// Entries are read one at a time: an entry of a kind this version does not
/// know, or one that does not parse, is left out and logged, and the rest of
/// the registry is still usable.
/// An entry without a `type` is a `command`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(from = "RawRegistry")]
pub struct Registry {
    /// Schema version.
    /// Currently `1`.
    pub version: u32,

    /// Map of command path to plugin metadata.
    ///
    /// Keys are space-separated command paths (e.g. `"serve"`, `"serve web"`).
    /// Each key corresponds to a `jp` subcommand.
    pub plugins: BTreeMap<String, RegistryPlugin>,
}

/// The registry as it arrives, before its entries are read.
#[derive(Deserialize)]
struct RawRegistry {
    version: u32,
    #[serde(default)]
    plugins: BTreeMap<String, serde_json::Value>,
}

impl From<RawRegistry> for Registry {
    fn from(raw: RawRegistry) -> Self {
        let mut plugins = BTreeMap::new();

        for (key, mut entry) in raw.plugins {
            // A `type` that is not a string is left for parsing to reject.
            if let Some(object) = entry.as_object_mut()
                && let Some(kind) = object
                    .entry("type")
                    .or_insert_with(|| "command".into())
                    .as_str()
                && !KNOWN_KINDS.contains(&kind)
            {
                debug!(
                    key,
                    kind, "Registry entry of a kind this jp does not know; ignored."
                );
                continue;
            }

            match serde_json::from_value(entry) {
                Ok(plugin) => {
                    plugins.insert(key, plugin);
                }
                Err(error) => warn!(key, %error, "Malformed registry entry; ignored."),
            }
        }

        Self {
            version: raw.version,
            plugins,
        }
    }
}

/// The `type` values [`PluginKind`] reads.
const KNOWN_KINDS: [&str; 2] = ["command", "command_group"];

/// A plugin entry in the registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegistryPlugin {
    /// Stable identifier used for binary naming, config keys, and install
    /// paths.
    /// The binary is `jp-{id}`, config lives at `plugins.command.{id}`, and the
    /// install path is `$XDG_DATA_HOME/jp/plugins/command/jp-{id}`.
    pub id: String,

    /// One-line description shown in `jp -h` and `jp plugin list`.
    pub description: String,

    /// Whether this is an official JP plugin (auto-installed without
    /// prompting).
    #[serde(default)]
    pub official: bool,

    /// Repository URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,

    /// Kind-specific fields.
    #[serde(flatten)]
    pub kind: PluginKind,
}

/// Plugin kind, carrying variant-specific fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginKind {
    /// A standalone binary communicating over the JSON-lines protocol.
    Command {
        /// Command paths (registry keys) of plugins that must be installed for
        /// this one to work.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        requires: Vec<String>,

        /// Command paths (registry keys) of plugins that extend this one with
        /// additional subcommands.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        suggests: Vec<String>,

        /// Platform-specific downloadable binaries, keyed by target triple
        /// (e.g. `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`).
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        binaries: BTreeMap<String, RegistryBinary>,
    },

    /// A command namespace with no binary.
    /// Provides help text and lists sub-plugins via `suggests`.
    /// `jp <group>` prints help and exits with code 2.
    /// `jp <group> <sub>` dispatches to the sub-plugin.
    CommandGroup {
        /// Command paths (registry keys) of plugins that extend this group with
        /// additional subcommands.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        suggests: Vec<String>,
    },
}

impl Default for PluginKind {
    fn default() -> Self {
        Self::Command {
            requires: vec![],
            suggests: vec![],
            binaries: BTreeMap::new(),
        }
    }
}

impl PluginKind {
    /// Returns `true` if this is a [`PluginKind::Command`].
    #[must_use]
    pub fn is_command(&self) -> bool {
        matches!(self, Self::Command { .. })
    }

    /// Returns `true` if this is a [`PluginKind::CommandGroup`].
    #[must_use]
    pub fn is_command_group(&self) -> bool {
        matches!(self, Self::CommandGroup { .. })
    }

    /// Returns the `binaries` map if this is a `Command`, or an empty map
    /// otherwise.
    #[must_use]
    pub fn binaries(&self) -> &BTreeMap<String, RegistryBinary> {
        static EMPTY: BTreeMap<String, RegistryBinary> = BTreeMap::new();

        match self {
            Self::Command { binaries, .. } => binaries,
            Self::CommandGroup { .. } => &EMPTY,
        }
    }

    /// Returns a mutable reference to the `binaries` map.
    ///
    /// Returns `None` for non-`Command` variants.
    pub fn binaries_mut(&mut self) -> Option<&mut BTreeMap<String, RegistryBinary>> {
        match self {
            Self::Command { binaries, .. } => Some(binaries),
            Self::CommandGroup { .. } => None,
        }
    }
}

/// A downloadable binary for a specific platform target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegistryBinary {
    /// Download URL for the binary.
    pub url: String,

    /// Expected SHA-256 hex digest of the binary.
    pub sha256: String,
}

/// Locally stored plugin approval records.
///
/// Remembers the user's answers to the `ask` prompt, and which binaries JP
/// installed itself.
/// Stored at `$XDG_DATA_HOME/jp/plugin-approvals.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PluginApprovals {
    /// Approvals keyed by plugin name, the same key as
    /// `plugins.command.<name>`.
    #[serde(default)]
    pub approved: BTreeMap<String, ApprovedPlugin>,
}

/// One approved plugin binary: that file, with those contents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovedPlugin {
    /// Absolute path to the approved binary.
    pub path: Utf8PathBuf,

    /// SHA-256 hex digest of the binary at time of approval.
    pub sha256: String,

    /// When the approval was recorded.
    pub approved_at: DateTime<Utc>,

    /// Whether JP wrote this binary itself, from the registry.
    ///
    /// JP updates an official binary it installed, and leaves alone one that
    /// was changed on this machine or approved by the user.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub installed: bool,

    /// The manifest fields the binary's `describe` answer gave, for a binary
    /// whose file carries no readable manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Manifest>,
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
