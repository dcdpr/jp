//! `jp plugin update` subcommand.

use jp_printer::Printer;

use super::registry;
use crate::{Ctx, cmd};

/// Refresh the plugin registry cache and check for updates.
#[derive(Debug, clap::Args)]
pub(crate) struct Update;

impl Update {
    #[allow(clippy::unused_self)]
    pub(crate) async fn run(&self, ctx: &Ctx) -> cmd::Output {
        let printer = &ctx.printer;

        printer.eprintln("  \u{2192} Refreshing plugin registry...");
        let client = reqwest::Client::new();
        let reg = registry::fetch(&client).await?;

        registry::save_cache(&reg)?;
        printer.eprintln(format!(
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
        let mut outdated = Vec::new();

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
                outdated.push(name.clone());
            }
        }

        report_updates(printer, &outdated);

        Ok(())
    }
}

/// Report which installed plugins have a newer binary in the registry.
///
/// `outdated` names the plugins whose installed binary no longer matches the
/// registry's checksum.
/// An empty slice reports that everything is current, so callers skip the call
/// when nothing is installed at all.
fn report_updates(printer: &Printer, outdated: &[String]) {
    for name in outdated {
        printer.eprintln(format!("  \u{2192} {name}: update available"));
    }

    if outdated.is_empty() {
        printer.eprintln("  \u{2192} All installed plugins are up to date.");
    }
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
