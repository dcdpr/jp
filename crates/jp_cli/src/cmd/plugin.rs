//! Plugin management and dispatch.
//!
//! This module handles:
//! - `jp plugin list|install|update` management subcommands
//! - External plugin dispatch (spawning `jp-<name>` binaries)
//!
//! See: `docs/rfd/D17-command-plugin-system.md`

pub(crate) mod dispatch;
mod install;
mod list;
pub(crate) mod registry;
mod update;

use crate::{Ctx, cmd};

/// `jp plugin` subcommand group for managing plugins.
#[derive(Debug, clap::Args)]
pub(crate) struct PluginManagement {
    #[command(subcommand)]
    command: PluginCmd,
}

#[derive(Debug, clap::Subcommand)]
enum PluginCmd {
    /// List available and installed plugins.
    #[command(visible_alias = "ls")]
    List(list::List),

    /// Install a plugin from the registry.
    Install(install::Install),

    /// Refresh the plugin registry cache and check for updates.
    Update(update::Update),
}

impl PluginManagement {
    pub(crate) async fn run(&self, ctx: &Ctx) -> cmd::Output {
        match &self.command {
            PluginCmd::List(cmd) => cmd.run(),
            PluginCmd::Install(cmd) => cmd.run(ctx).await,
            PluginCmd::Update(cmd) => cmd.run(ctx).await,
        }
    }
}
