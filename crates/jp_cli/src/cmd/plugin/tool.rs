//! Admitting the command plugins a turn's tools run through.
//!
//! A turn whose tools run through command plugins (`source =
//! "command.<plugin>"`) admits each of those plugins once, before the turn
//! starts, with [`TurnPlugins::admit`]: the same trust decision `jp <plugin>`
//! makes, asked at a point where a prompt cannot open inside a tool call.
//! A plugin that is not admitted takes its tools out of the turn
//! ([`without_refused_plugins`]), and the turn says so
//! ([`report_refused_plugins`]).
//!
//! The admitted plugins go to the tool service as [`CommandPlugins`], which
//! runs each call: it starts the admitted binary with an `init` carrying the
//! call and the plugin's options from the turn's configuration, and reads back
//! one `tool_outcome`.
//! The plugin resolves no configuration of its own, so what it sees is what the
//! query making the call sees: nested `.jp.toml` files, conversation config,
//! and `--cfg` overrides included.
//!
//! See: `docs/rfd/072-command-plugin-system.md`, "Tool Calls".

use std::collections::{BTreeSet, HashMap};

use crossterm::style::Stylize as _;
use indexmap::IndexMap;
use jp_config::{
    conversation::tool::{ToolConfigWithDefaults, ToolSource, ToolsConfig},
    plugins::PluginsConfig,
};
use jp_mcp::server::{AdmittedPlugin, CommandPlugins, PluginInit, is_offered};
use jp_plugin::registry::Registry;
use jp_printer::{PrintableExt as _, Printer};
use jp_tool::Error as ToolError;
use tracing::{debug, warn};

use super::{
    admission::{Official, admit},
    approvals::ApprovalStore,
    discovery::LocalPlugin,
    dispatch::{local_plugins, plugin_options},
    registry, routing,
};

/// The plugins the offered tools of a turn run through.
///
/// Mirrors which tools the turn offers the assistant: an enabled tool, or the
/// tool `forced_tool` names unless it is locked off.
pub(crate) fn plugins_needed(tools: &ToolsConfig, forced_tool: Option<&str>) -> BTreeSet<String> {
    tools
        .iter()
        .filter(|(name, config)| is_offered(name, config, forced_tool))
        .filter_map(|(_, config)| match config.source() {
            ToolSource::Command { plugin, .. } => Some(plugin.clone()),
            _ => None,
        })
        .collect()
}

/// What admission decided about each plugin a turn's tools run through.
#[derive(Debug, Default)]
pub(crate) struct TurnPlugins {
    admitted: HashMap<String, AdmittedPlugin>,

    /// Plugins that may not run this turn, and why, in the order they were
    /// decided.
    refused: IndexMap<String, String>,
}

impl TurnPlugins {
    /// Admit each plugin in `needed`, the way `jp <plugin>` admits one.
    ///
    /// With `interactive`, a binary nobody approved is asked about; without, it
    /// is refused.
    /// Nothing is installed: an official plugin that is missing is refused, not
    /// fetched as the turn starts.
    pub(crate) fn admit(
        needed: &BTreeSet<String>,
        plugins_config: &PluginsConfig,
        interactive: bool,
        printer: &Printer,
    ) -> Self {
        // Discovery reads every `jp-*` binary on `$PATH`; a turn with no
        // plugin tools has no reason to.
        if needed.is_empty() {
            return Self::default();
        }

        let mut approvals = ApprovalStore::load();
        let local = local_plugins(&approvals);
        let registry = registry::load_cached();

        Self::admit_from(
            needed,
            &local,
            registry.as_ref(),
            plugins_config,
            &mut approvals,
            interactive,
            printer,
        )
    }

    /// [`Self::admit`], with the binaries, the registry, and the approvals
    /// passed in.
    fn admit_from(
        needed: &BTreeSet<String>,
        local: &[LocalPlugin],
        registry: Option<&Registry>,
        plugins_config: &PluginsConfig,
        approvals: &mut ApprovalStore,
        interactive: bool,
        printer: &Printer,
    ) -> Self {
        let mut plugins = Self::default();

        for name in needed {
            match admit_one(
                name,
                local,
                registry,
                plugins_config,
                approvals,
                interactive,
                printer,
            ) {
                Ok(admitted) => {
                    debug!(plugin = name, binary = %admitted.binary, "Plugin admitted for the turn.");
                    plugins.admitted.insert(name.clone(), admitted);
                }
                Err(reason) => {
                    warn!(plugin = name, %reason, "Plugin not admitted; its tools are left out.");
                    plugins.refused.insert(name.clone(), reason);
                }
            }
        }

        plugins
    }

    /// The plugins that may not run this turn, and why.
    pub(crate) fn refused(&self) -> &IndexMap<String, String> {
        &self.refused
    }

    /// The admitted plugins, as the tool service runs them, with `init` shared
    /// by every call.
    pub(crate) fn into_command_plugins(self, init: PluginInit) -> CommandPlugins {
        self.admitted
            .into_iter()
            .fold(CommandPlugins::new(init), |plugins, (name, admitted)| {
                plugins.with(name, admitted)
            })
    }
}

/// Admit one plugin by name.
///
/// The error is the reason, written for the user.
fn admit_one(
    name: &str,
    local: &[LocalPlugin],
    registry: Option<&Registry>,
    plugins_config: &PluginsConfig,
    approvals: &mut ApprovalStore,
    interactive: bool,
    printer: &Printer,
) -> Result<AdmittedPlugin, String> {
    let named = routing::by_name(name, local, registry)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!("no `jp-{name}` binary in the plugin install directory or on $PATH")
        })?;

    let official = Official {
        official: named.official.is_some(),
        sha256: named
            .official
            .and_then(registry::release)
            .map(|release| release.sha256.as_str()),
        replaces: None,
    };

    // The digest admission decided on, not a second read of the file: a binary
    // replaced while the prompt was open must fail the per-call check, not
    // become the turn's baseline.
    let sha256 = admit(
        named.plugin,
        official,
        plugins_config,
        approvals,
        interactive,
        printer,
    )
    .map_err(error_message)?;

    Ok(AdmittedPlugin {
        binary: named.plugin.path.clone(),
        sha256,
        options: plugin_options(plugins_config, name),
    })
}

/// The sentence a command error carries, without its exit code.
fn error_message(error: crate::cmd::Error) -> String {
    error
        .message
        .unwrap_or_else(|| format!("failed with exit code {}", error.code))
}

/// The turn's tools, without those whose plugin was refused.
///
/// # Errors
///
/// [`ToolError::CommandPluginUnavailable`] when `forced_tool` names one of
/// them: the user asked for that tool, and a turn without it is not what they
/// asked for.
pub(crate) fn without_refused_plugins<'a>(
    tools: impl Iterator<Item = (&'a str, ToolConfigWithDefaults)>,
    refused: &IndexMap<String, String>,
    forced_tool: Option<&str>,
) -> Result<Vec<(&'a str, ToolConfigWithDefaults)>, ToolError> {
    let mut usable = Vec::new();

    for (name, config) in tools {
        if let ToolSource::Command { plugin, .. } = config.source()
            && let Some(reason) = refused.get(plugin)
        {
            if forced_tool == Some(name) {
                return Err(ToolError::CommandPluginUnavailable {
                    plugin: plugin.clone(),
                    reason: reason.clone(),
                });
            }
            continue;
        }

        usable.push((name, config));
    }

    Ok(usable)
}

/// Report the plugins a turn left out, and the tools that went with them.
///
/// Emitted whatever the progress settings say, as a skipped MCP server is: a
/// tool silently missing from the turn is the failure this reports.
pub(crate) fn report_refused_plugins(
    printer: &Printer,
    tools: &ToolsConfig,
    refused: &IndexMap<String, String>,
    forced_tool: Option<&str>,
) {
    for (plugin, reason) in refused {
        let names: Vec<String> = tools
            .iter()
            .filter(|(name, config)| is_offered(name, config, forced_tool))
            .filter(|(_, config)| {
                matches!(config.source(), ToolSource::Command { plugin: p, .. } if p == plugin)
            })
            .map(|(name, _)| name.to_owned())
            .collect();

        if printer.format().is_json() {
            let record = serde_json::json!({
                "event": "plugin_unavailable",
                "plugin": plugin,
                "reason": reason,
                "tools": names,
            });
            let line = if printer.format().is_json_pretty() {
                serde_json::to_string_pretty(&record)
            } else {
                serde_json::to_string(&record)
            }
            .unwrap_or_else(|_| record.to_string());
            printer.println_raw(line.to_err());
            continue;
        }

        let mut line = format!("Plugin '{plugin}' did not run");
        if !names.is_empty() {
            line.push_str("; unavailable tools: ");
            line.push_str(&names.join(", "));
        }
        printer.eprintln(line.yellow().to_string());
        printer.eprintln(format!("  {reason}"));
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
