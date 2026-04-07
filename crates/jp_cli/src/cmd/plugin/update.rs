//! `jp plugin update` subcommand.

use std::io::Write as _;

use super::registry;
use crate::{Ctx, cmd};

/// Refresh the plugin registry cache and check for updates.
#[derive(Debug, clap::Args)]
pub(crate) struct Update;

impl Update {
    #[allow(clippy::unused_self)]
    pub(crate) async fn run(&self, _ctx: &Ctx) -> cmd::Output {
        let mut err = std::io::stderr();

        drop(writeln!(err, "  \u{2192} Refreshing plugin registry..."));
        let client = reqwest::Client::new();
        let reg = registry::fetch(&client).await?;

        registry::save_cache(&reg)?;
        drop(writeln!(
            err,
            "  \u{2192} Registry updated ({} plugin{}).",
            reg.plugins.len(),
            if reg.plugins.len() == 1 { "" } else { "s" }
        ));

        // Check installed plugins for available updates.
        let installed = registry::discover_installed();
        if installed.is_empty() {
            return Ok(());
        }

        let target = registry::current_target();
        let mut any_updates = false;

        for (name, path) in &installed {
            // Installed plugins are stored by id. Find the matching
            // registry entry.
            let Some(plugin) = reg.plugins.values().find(|p| p.id == *name) else {
                continue;
            };
            let Some(binary) = plugin.kind.binaries().get(&target) else {
                continue;
            };
            let Ok(current_sha) = registry::sha256_file(path) else {
                continue;
            };
            if current_sha != binary.sha256 {
                drop(writeln!(err, "  \u{2192} {name}: update available"));
                any_updates = true;
            }
        }

        if !any_updates {
            drop(writeln!(
                err,
                "  \u{2192} All installed plugins are up to date."
            ));
        }

        Ok(())
    }
}
