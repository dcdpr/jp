//! `jp plugin uninstall` subcommand.

use std::fs;

use jp_printer::Printer;

use super::{approvals::ApprovalStore, discovery, registry};
use crate::cmd;

/// Remove a plugin JP installed, and its approval.
///
/// A binary on `$PATH` belongs to whatever put it there, and is left in place.
#[derive(Debug, clap::Args)]
pub(crate) struct Uninstall {
    /// The plugin's name: its file name without `jp-`.
    name: String,
}

impl Uninstall {
    pub(crate) fn run(&self, printer: &Printer) -> cmd::Output {
        let name = &self.name;

        let Some(path) = registry::find_installed(name) else {
            let on_path = discovery::discover()
                .into_iter()
                .find(|plugin| plugin.name == *name);

            return Err(match on_path {
                Some(plugin) => format!(
                    "`jp-{name}` at {} was not installed by JP, so it is left in place. Remove it \
                     with whatever installed it, or run `jp plugin revoke {name}` to withdraw its \
                     approval.",
                    plugin.path
                ),
                None => format!("no plugin named `{name}` is installed"),
            }
            .into());
        };

        let mut approvals = ApprovalStore::load();
        let approved_here = approvals
            .get(name)
            .and_then(|approval| approval.path.canonicalize_utf8().ok())
            .zip(path.canonicalize_utf8().ok())
            .is_some_and(|(approved, installed)| approved == installed);

        fs::remove_file(&path).map_err(|e| format!("failed to remove {path}: {e}"))?;

        // The approval names this file; one for another binary with the same
        // name is not this command's to remove.
        if approved_here {
            approvals.remove(name)?;
        }

        printer.eprintln(format!("  \u{2192} Removed {path}"));
        Ok(())
    }
}
