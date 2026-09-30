//! `jp plugin list` subcommand.

use std::{
    fmt::Write as _,
    io::{self, Write as _},
};

use jp_plugin::registry::{PluginKind, Registry};
use jp_printer::Printer;

use super::{
    approvals::{ApprovalMatch, ApprovalStore},
    discovery::LocalPlugin,
    dispatch::local_plugins,
    registry::{self, Fetched},
    routing::official_entries,
};
use crate::cmd;

/// List installed plugins, and the ones the registry offers.
#[derive(Debug, clap::Args)]
pub(crate) struct List;

impl List {
    /// List the plugins.
    ///
    /// Fetches the registry first, because asking for the list is asking what
    /// is published now.
    /// When the fetch fails, the copy from the last fetch is used and the
    /// listing says so; with none, only installed plugins are listed.
    #[allow(clippy::unnecessary_wraps, clippy::unused_self)]
    pub(crate) async fn run(&self, printer: &Printer) -> cmd::Output {
        let approvals = ApprovalStore::load();
        let local = local_plugins(&approvals);

        let registry = match registry::fetch_or_load(&registry::client()).await {
            Ok(Fetched::Fresh(registry)) => Some(registry),
            Ok(Fetched::Cached(registry)) => {
                printer.eprintln(
                    "  \u{2192} Could not reach the plugin registry; showing the copy from the \
                     last fetch.",
                );
                Some(registry)
            }
            Err(error) => {
                printer.eprintln(format!(
                    "  \u{2192} Could not reach the plugin registry ({}); listing installed \
                     plugins only.",
                    error.message.as_deref().unwrap_or("fetch failed"),
                ));
                None
            }
        };

        let rows: Vec<_> = local
            .iter()
            .map(|plugin| {
                let release = registry.as_ref().and_then(|registry| {
                    official_entries(Some(registry))
                        .into_values()
                        .find(|entry| entry.kind.is_command() && entry.id == plugin.name)
                        .and_then(registry::release)
                        .map(|release| release.sha256.clone())
                });

                let state = match registry::sha256_file(&plugin.path) {
                    Ok(sha256) => approval_state(
                        release.as_deref(),
                        &sha256,
                        &approvals.check(&plugin.name, &plugin.path, &sha256),
                        approvals.get(&plugin.name).is_some_and(|a| a.installed),
                    ),
                    Err(_) => "unreadable".to_owned(),
                };

                (plugin, state)
            })
            .collect();

        drop(write!(
            io::stdout().lock(),
            "{}",
            render(&rows, registry.as_ref())
        ));

        Ok(())
    }
}

/// Whether a binary runs without asking, and why.
///
/// `release` is the SHA-256 the registry publishes for an official plugin.
fn approval_state(
    release: Option<&str>,
    sha256: &str,
    approval: &ApprovalMatch,
    installed: bool,
) -> String {
    match (release, approval) {
        (Some(release), _) if release == sha256 => "official release",
        (_, ApprovalMatch::Matches) if installed => "installed by jp",
        (_, ApprovalMatch::Matches) => "approved",
        (_, ApprovalMatch::Changed) => "changed since approved",
        (_, ApprovalMatch::Elsewhere(_)) => "another binary is approved",
        (Some(_), ApprovalMatch::None) => "not the official release",
        (None, ApprovalMatch::None) => "not approved",
    }
    .to_owned()
}

/// The listing: every plugin binary on this machine, then what the registry
/// offers that is not installed.
fn render(rows: &[(&LocalPlugin, String)], registry: Option<&Registry>) -> String {
    let official = official_entries(registry);
    let is_official = |name: &str| {
        official
            .values()
            .any(|entry| entry.kind.is_command() && entry.id == name)
    };

    let mut out = String::new();

    if !rows.is_empty() {
        out.push_str("Installed:\n");
    }

    for (plugin, state) in rows {
        let command = plugin
            .manifest
            .valid()
            .map(|manifest| manifest.command.join(" "));

        let mut notes = vec![if is_official(&plugin.name) {
            "official".to_owned()
        } else {
            "third-party".to_owned()
        }];

        notes.push(state.clone());

        if !is_official(&plugin.name)
            && let Some(command) = &command
            && official.contains_key(command.as_str())
        {
            notes.push(format!("replaces the official `jp {command}`"));
        }

        notes.extend(plugin.manifest.problem());

        if rows.iter().filter(|(p, _)| p.name == plugin.name).count() > 1 {
            notes.push("another binary has the same name".to_owned());
        }

        let command = command.map_or_else(|| "-".to_owned(), |c| format!("jp {c}"));
        let _ = writeln!(
            out,
            "  {:<16} {:<20} {} ({})",
            plugin.name,
            command,
            plugin.path,
            notes.join(", ")
        );
    }

    // A group has no binary to install, so it is always listed here.
    let available: Vec<_> = registry
        .into_iter()
        .flat_map(|registry| registry.plugins.iter())
        .filter(|(_, entry)| {
            entry.kind.is_command_group() || !rows.iter().any(|(p, _)| p.name == entry.id)
        })
        .collect();

    if !available.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("Available:\n");
    }

    for (key, entry) in available {
        let how = match (&entry.kind, entry.official) {
            (PluginKind::CommandGroup { .. }, true) => "command group, official".to_owned(),
            (PluginKind::CommandGroup { .. }, false) => "command group, third-party".to_owned(),
            (PluginKind::Command { .. }, true) => {
                "command, official, installs on first use".to_owned()
            }
            (PluginKind::Command { .. }, false) => {
                format!("command, third-party, `jp plugin install {}`", entry.id)
            }
        };

        let _ = writeln!(
            out,
            "  {:<16} {:<20} {} ({how})",
            entry.id,
            format!("jp {key}"),
            entry.description,
        );
    }

    if out.is_empty() {
        out.push_str("No plugins found. Run `jp plugin update` to fetch the registry.\n");
    }

    out
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
