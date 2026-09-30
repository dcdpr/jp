//! Plugin management and dispatch.
//!
//! This module handles:
//!
//! - `jp plugin list|install|uninstall|update|approve|revoke` management
//!   subcommands, which need no workspace
//! - External plugin dispatch: routing an unknown subcommand to the plugin
//!   whose manifest claims it, and spawning it
//!
//! See: `docs/rfd/072-command-plugin-system.md`

mod admission;
mod approvals;
mod approve;
pub(crate) mod discovery;
pub(crate) mod dispatch;
pub(crate) mod help;
mod install;
mod list;
mod output;
mod process_tree;
pub(crate) mod registry;
mod revoke;
pub(crate) mod routing;
mod uninstall;
mod update;

use jp_printer::Printer;
use tokio_util::sync::CancellationToken;

use crate::{KeyValueOrPath, cmd};

/// `jp plugin` subcommand group for managing plugins.
#[derive(Debug, clap::Args)]
pub(crate) struct PluginManagement {
    #[command(subcommand)]
    command: PluginCmd,
}

#[derive(Debug, clap::Subcommand)]
enum PluginCmd {
    /// List installed plugins, and the ones the registry offers.
    #[command(visible_alias = "ls")]
    List(list::List),

    /// Install a plugin from the registry, with the plugins it requires.
    Install(install::Install),

    /// Remove a plugin JP installed, and its approval.
    Uninstall(uninstall::Uninstall),

    /// Refresh the plugin registry, and update the plugins JP installed.
    Update(update::Update),

    /// Approve a plugin binary, so it runs without asking.
    Approve(approve::Approve),

    /// Withdraw a plugin's approval, so it asks again before it runs.
    Revoke(revoke::Revoke),
}

impl PluginManagement {
    /// Run a management command.
    ///
    /// Plugin binaries and the registry cache are user-global, so none of these
    /// needs a workspace, and they work from any directory.
    /// `cfg` is the invocation's `--cfg` arguments, layered over the
    /// user-global configuration where a command reads configuration.
    pub(crate) async fn run(
        &self,
        printer: &Printer,
        interactive: bool,
        cfg: &[KeyValueOrPath],
    ) -> cmd::Output {
        match &self.command {
            PluginCmd::List(cmd) => cmd.run(printer).await,
            PluginCmd::Install(cmd) => cmd.run(printer, interactive).await,
            PluginCmd::Uninstall(cmd) => cmd.run(printer),
            PluginCmd::Update(cmd) => cmd.run(printer, cfg).await,
            PluginCmd::Approve(cmd) => cmd.run(printer, cfg, &cancel_on_ctrl_c()),
            PluginCmd::Revoke(cmd) => cmd.run(printer),
        }
    }
}

/// A token the first Ctrl-C cancels.
///
/// `jp plugin` runs without the signal router, so Ctrl-C would otherwise end
/// `jp` at once, before it could stop a plugin it is running.
/// Listening also takes the signal's default action away for the rest of the
/// process, so only a command that watches the token may ask for one.
fn cancel_on_ctrl_c() -> CancellationToken {
    let token = CancellationToken::new();
    let cancel = token.clone();

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel.cancel();
        }
    });

    token
}
