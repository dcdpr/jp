//! `jp plugin update` subcommand.
//!
//! Fetches the registry, then brings every installed plugin the registry
//! publishes up to its current release where JP may replace it, and says why
//! for the ones it leaves alone.
//! This is the only place an installed plugin is updated: running a plugin
//! never reaches the network.

use jp_config::PartialAppConfig;
use jp_plugin::registry::{Registry, RegistryPlugin};
use jp_printer::Printer;

use super::{
    approvals::{ApprovalMatch, ApprovalStore},
    discovery::{self, LocalPlugin, Location},
    install, registry,
};
use crate::{KeyValueOrPath, cmd, load_user_global_partial};

/// Refresh the plugin registry, and update the plugins JP installed.
#[derive(Debug, clap::Args)]
pub(crate) struct Update;

impl Update {
    #[allow(clippy::unused_self)]
    pub(crate) async fn run(&self, printer: &Printer, cfg: &[KeyValueOrPath]) -> cmd::Output {
        printer.eprintln("  \u{2192} Refreshing plugin registry...");
        let client = registry::client();
        let reg = registry::fetch(&client).await?;

        printer.eprintln(format!(
            "  \u{2192} Registry updated ({} plugin{}).",
            reg.plugins.len(),
            if reg.plugins.len() == 1 { "" } else { "s" }
        ));

        // Outside any workspace, a pin comes from the user-global configuration
        // with `--cfg` on top.
        let config = load_user_global_partial(cfg)?;
        let mut approvals = ApprovalStore::load();
        let mut current = true;
        let mut failed = Vec::new();

        for (plugin, entry) in published(&discovery::discover(), &reg) {
            let Some(release) = registry::release(entry) else {
                continue;
            };
            let Ok(sha256) = registry::sha256_file(&plugin.path) else {
                continue;
            };

            let decision = decide(&Candidate {
                official: entry.official,
                location: plugin.location,
                sha256: &sha256,
                release_sha256: &release.sha256,
                pinned: is_pinned(&config, &plugin.name),
                approval: approvals.check(&plugin.name, &plugin.path, &sha256),
                installed_by_jp: approvals.get(&plugin.name).is_some_and(|a| a.installed),
            });

            match decision {
                Decision::Current => {}
                // One plugin failing to download is no reason to leave the rest
                // out of date.
                Decision::Update => {
                    current = false;
                    if let Err(error) =
                        install::download(&client, entry, &mut approvals, printer).await
                    {
                        printer.error_println(format!(
                            "  \u{2192} {}: update failed: {}",
                            plugin.name,
                            error.message.as_deref().unwrap_or("download failed"),
                        ));
                        failed.push(plugin.name.clone());
                    }
                }
                Decision::Hold(hold) => {
                    current = false;
                    printer.eprintln(format!(
                        "  \u{2192} {}: update available, not installed ({})",
                        plugin.name,
                        hold.reason(&plugin.name),
                    ));
                }
            }
        }

        if current {
            printer.eprintln("  \u{2192} All installed plugins are up to date.");
        }

        if !failed.is_empty() {
            return Err(format!("failed to update: {}", failed.join(", ")).into());
        }

        Ok(())
    }
}

/// The installed plugins the registry publishes a command for, with the entry
/// that publishes it.
fn published<'a>(
    local: &'a [LocalPlugin],
    reg: &'a Registry,
) -> impl Iterator<Item = (&'a LocalPlugin, &'a RegistryPlugin)> {
    local.iter().filter_map(|plugin| {
        reg.plugins
            .values()
            .find(|entry| entry.kind.is_command() && entry.id == plugin.name)
            .map(|entry| (plugin, entry))
    })
}

/// Whether configuration pins the plugin to a checksum.
fn is_pinned(config: &PartialAppConfig, name: &str) -> bool {
    config
        .plugins
        .command
        .get(name)
        .is_some_and(|c| c.checksum.is_some())
}

/// What [`decide`] knows about one installed plugin.
#[derive(Debug)]
struct Candidate<'a> {
    /// Whether the registry entry is official.
    official: bool,

    /// Where the binary is.
    location: Location,

    /// The SHA-256 of the installed binary.
    sha256: &'a str,

    /// The SHA-256 of the release the registry publishes for this platform.
    release_sha256: &'a str,

    /// Whether configuration pins the binary to a checksum.
    pinned: bool,

    /// How the binary relates to the approval stored for its name.
    approval: ApprovalMatch,

    /// Whether that approval records JP installing the binary.
    installed_by_jp: bool,
}

/// What `jp plugin update` does with one installed plugin.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// The installed binary is the current release.
    Current,

    /// Replace it with the current release.
    Update,

    /// A newer release exists, and this binary is not JP's to replace.
    Hold(Hold),
}

/// Why a binary with a newer release is left alone.
#[derive(Debug, PartialEq, Eq)]
enum Hold {
    /// Third-party plugins are never updated for the user.
    ThirdParty,

    /// Configuration pins the binary to a checksum.
    Pinned,

    /// Something other than JP put the binary there, such as a package manager.
    NotInstalledByJp,

    /// The binary changed since JP installed it.
    Changed,
}

impl Hold {
    fn reason(&self, name: &str) -> String {
        match self {
            Self::ThirdParty => "third-party plugins are not updated automatically".to_owned(),
            Self::Pinned => format!("pinned by plugins.command.{name}.checksum"),
            Self::NotInstalledByJp => "jp did not install it".to_owned(),
            Self::Changed => "it changed since jp installed it".to_owned(),
        }
    }
}

/// Decide what to do with one installed plugin.
///
/// JP replaces only an official binary in its own install directory that is
/// still the one it wrote, and that no pin holds in place.
fn decide(candidate: &Candidate<'_>) -> Decision {
    if candidate.sha256 == candidate.release_sha256 {
        return Decision::Current;
    }

    let hold = if !candidate.official {
        Hold::ThirdParty
    } else if candidate.pinned {
        Hold::Pinned
    } else if candidate.location != Location::InstallDir || !candidate.installed_by_jp {
        Hold::NotInstalledByJp
    } else if candidate.approval != ApprovalMatch::Matches {
        Hold::Changed
    } else {
        return Decision::Update;
    };

    Decision::Hold(hold)
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
