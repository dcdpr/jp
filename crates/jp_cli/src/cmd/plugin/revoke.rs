//! `jp plugin revoke` subcommand.

use clap::{ArgMatches, Command};
use jp_printer::Printer;

use super::{approvals::ApprovalStore, plugin_name_arg};
use crate::{
    cli::{CommandArgs, required},
    cmd,
};

/// Withdraw a plugin's approval, so it asks again before it runs.
#[derive(Debug)]
pub(crate) struct Revoke {
    /// The plugin's name: its file name without `jp-`.
    name: String,
}

impl CommandArgs for Revoke {
    fn augment(cmd: Command) -> Command {
        cmd.arg(plugin_name_arg(
            "The plugin's name: its file name without `jp-`",
        ))
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            name: required(matches, "name")?,
        })
    }
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
