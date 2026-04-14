//! `jp plugin list` subcommand.

use std::{
    collections::HashSet,
    io::{self, Write as _},
};

use super::{dispatch, registry};
use crate::cmd;

/// List available and installed plugins.
#[derive(Debug, clap::Args)]
pub(crate) struct List;

impl List {
    #[allow(clippy::unnecessary_wraps, clippy::unused_self)]
    pub(crate) fn run(&self) -> cmd::Output {
        let mut out = io::stdout().lock();
        let cached = registry::load_cached();
        let installed = registry::discover_installed();
        let path_plugins = dispatch::discover_plugins();

        let installed_names: HashSet<&str> = installed.iter().map(|(n, _)| n.as_str()).collect();

        let mut any_output = false;

        // Registry plugins
        if let Some(ref reg) = cached
            && !reg.plugins.is_empty()
        {
            drop(writeln!(out, "Registry:"));
            for (cmd_path, plugin) in &reg.plugins {
                let status = if installed_names.contains(plugin.id.as_str()) {
                    "installed"
                } else {
                    "available"
                };
                let badge = if plugin.official { " (official)" } else { "" };
                drop(writeln!(
                    out,
                    "  {cmd_path:<20} {:<48} [{status}]{badge}",
                    plugin.description
                ));
            }
            any_output = true;
        }

        // PATH plugins (exclude those already in the install dir, since
        // discover_plugins also scans the install dir)
        let path_only: Vec<_> = path_plugins
            .iter()
            .filter(|(name, _)| !installed_names.contains(name.as_str()))
            .collect();

        if !path_only.is_empty() {
            if any_output {
                drop(writeln!(out));
            }
            drop(writeln!(out, "PATH:"));
            for (name, path) in path_only {
                drop(writeln!(out, "  {name:<20} {path}"));
            }
            any_output = true;
        }

        if !any_output {
            drop(writeln!(
                out,
                "No plugins found. Run `jp plugin update` to fetch the registry."
            ));
        }

        Ok(())
    }
}
