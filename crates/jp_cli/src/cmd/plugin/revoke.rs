//! `jp plugin revoke` subcommand.

use jp_printer::Printer;

use super::approvals::ApprovalStore;
use crate::cmd;

/// Withdraw a plugin's approval, so it asks again before it runs.
#[derive(Debug, clap::Args)]
pub(crate) struct Revoke {
    /// The plugin's name: its file name without `jp-`.
    name: String,
}

impl Revoke {
    pub(crate) fn run(&self, printer: &Printer) -> cmd::Output {
        let removed = ApprovalStore::load()
            .remove(&self.name)?
            .ok_or_else(|| format!("plugin `{}` has no approval to revoke", self.name))?;

        printer.eprintln(format!(
            "  \u{2192} Revoked the approval for `{}` ({}).",
            self.name, removed.path
        ));

        Ok(())
    }
}
