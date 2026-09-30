//! Plugin listings in help output.
//!
//! Everything here reads manifests and the registry cache and spawns nothing: a
//! plugin runs only once admission has passed it, and listing it in help is not
//! a reason to run it.
//!
//! See: `docs/rfd/072-command-plugin-system.md`, "Plugin Self-Description" and
//! "Help Aggregation for Command Groups".

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::{self, Write as _},
};

use jp_plugin::registry::Registry;

use super::{
    approvals::ApprovalStore,
    discovery::LocalPlugin,
    dispatch::local_plugins,
    registry::{self, Refresh},
    routing::{official_entries, segments},
};

/// One line of a plugin listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// What the user types after `jp`, or after the group's own path.
    pub name: String,

    /// What the listing says about it.
    pub description: String,
}

/// Print the "Plugins:" section that follows the built-in commands in `jp -h`.
///
/// Fetches the registry when there is none cached, so official commands are
/// listed from the first run, unless `JP_NO_PLUGIN_DOWNLOAD` is set.
pub(crate) fn print_root_section() {
    let local = local_plugins(&ApprovalStore::load());
    let registry = registry::load_cached().or_else(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?
            .block_on(registry::refresh(Refresh::WhenMissing))
    });

    let entries = root_entries(&local, registry.as_ref());
    if entries.is_empty() {
        return;
    }

    drop(write!(
        io::stdout().lock(),
        "\n{}",
        render_listing("Plugins:", &entries)
    ));
}

/// Every command a plugin provides, for `jp -h`.
///
/// A binary with a valid manifest is listed under the command it claims.
/// One without is listed by its file name, with the reason it claims nothing.
/// Official commands whose plugin is not installed are listed as if they were,
/// because typing one installs it.
pub(crate) fn root_entries(local: &[LocalPlugin], registry: Option<&Registry>) -> Vec<Entry> {
    let official = official_entries(registry);
    let mut entries = BTreeMap::new();

    for plugin in local {
        let (name, description) = match plugin.manifest.valid() {
            Some(manifest) => (manifest.command.join(" "), manifest.description.clone()),
            None => (
                plugin.name.clone(),
                format!("({})", plugin.manifest.problem().unwrap_or_default()),
            ),
        };

        entries.entry(name).or_insert(description);
    }

    for (key, entry) in official.iter().filter(|(_, e)| e.kind.is_command()) {
        entries
            .entry((*key).to_owned())
            .or_insert_with(|| entry.description.clone());
    }

    entries
        .into_iter()
        .map(|(name, description)| Entry { name, description })
        .collect()
}

/// The commands other plugins add under `path`, one per next segment.
///
/// `jp serve -h` lists `web` for a plugin claiming `serve web`, whether the
/// plugin is installed or only published.
/// An official one that is missing is marked `(not installed)`, since typing it
/// installs it; a third-party one names the command that installs it.
pub(crate) fn children(
    path: &[String],
    local: &[LocalPlugin],
    registry: Option<&Registry>,
) -> Vec<Entry> {
    // Filled in order of precedence, so an earlier source's description stays.
    let mut entries: BTreeMap<String, String> = BTreeMap::new();

    let mut add = |claim: &[&str], description: &str| {
        if claim.len() <= path.len() || !claim.iter().zip(path).all(|(c, p)| c == p) {
            return;
        }

        let child = claim[path.len()].to_owned();
        let described = claim.len() == path.len() + 1;
        let slot = entries.entry(child).or_default();
        if slot.is_empty() && described {
            description.clone_into(slot);
        }
    };

    for manifest in local.iter().filter_map(|plugin| plugin.manifest.valid()) {
        let claim: Vec<&str> = manifest.command.iter().map(String::as_str).collect();
        add(&claim, &manifest.description);
    }

    let installed = |id: &str| local.iter().any(|plugin| plugin.name == id);

    for (key, entry) in registry
        .into_iter()
        .flat_map(|registry| registry.plugins.iter())
        .filter(|(_, entry)| entry.kind.is_command())
    {
        let description = if installed(&entry.id) {
            entry.description.clone()
        } else if entry.official {
            format!("{} (not installed)", entry.description)
        } else {
            format!(
                "{} (third-party: `jp plugin install {}`)",
                entry.description, entry.id
            )
        };

        add(&segments(key), &description);
    }

    entries
        .into_iter()
        .map(|(name, description)| Entry { name, description })
        .collect()
}

/// A titled, aligned listing of entries.
pub(crate) fn render_listing(title: &str, entries: &[Entry]) -> String {
    let width = entries
        .iter()
        .map(|entry| entry.name.chars().count())
        .max()
        .unwrap_or_default()
        .max(14)
        + 2;

    let mut out = format!("{title}\n");
    for entry in entries {
        let _ = writeln!(
            out,
            "  {:<width$}{}",
            entry.name,
            entry.description,
            width = width
        );
    }

    out
}

/// The help for a command group, which has no binary of its own.
pub(crate) fn render_group(path: &[String], description: &str, entries: &[Entry]) -> String {
    let path = path.join(" ");
    let mut out = format!("{description}\n\nUsage: jp {path} <COMMAND>\n\n");

    if entries.is_empty() {
        out.push_str("No plugin provides a command here yet.\n");
    } else {
        out.push_str(&render_listing("Commands:", entries));
    }

    let _ = write!(
        out,
        "\nRun `jp {path} <command> -h` for more information.\n"
    );
    out
}

#[cfg(test)]
#[path = "help_tests.rs"]
mod tests;
