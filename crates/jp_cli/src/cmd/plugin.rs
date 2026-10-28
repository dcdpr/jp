//! Plugin management and dispatch.
//!
//! This module handles:
//!
//! - `jp plugin list|install|uninstall|update|approve|revoke` management
//!   subcommands, which need no workspace
//! - External plugin dispatch: routing an unknown subcommand to the plugin
//!   whose manifest claims it, and spawning it
//! - Tool calls served by plugins (`source = "plugin.command.<name>"`)
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
pub(crate) mod tool;
mod uninstall;
mod update;

use jp_printer::Printer;
use tokio_util::sync::CancellationToken;
use tracing::warn;

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
            PluginCmd::Approve(cmd) => cmd.run(printer, cfg, &cancel_on_shutdown_signal()),
            PluginCmd::Revoke(cmd) => cmd.run(printer),
        }
    }
}

/// A token the first shutdown signal cancels: Ctrl-C, or SIGTERM (Ctrl-Break on
/// Windows).
///
/// The same signals the signal router treats as a graceful shutdown.
/// `jp plugin` runs without the router, so either signal would otherwise end
/// `jp` at once, before it could stop a plugin it is running.
/// Listening also takes the signals' default action away for the rest of the
/// process, so only a command that watches the token may ask for one.
fn cancel_on_shutdown_signal() -> CancellationToken {
    let token = CancellationToken::new();
    cancel_when(&token, shutdown_requested());
    token
}

/// Cancel `token` once `request` completes.
fn cancel_when(token: &CancellationToken, request: impl Future<Output = ()> + Send + 'static) {
    let cancel = token.clone();

    tokio::spawn(async move {
        request.await;
        cancel.cancel();
    });
}

/// Completes on the first Ctrl-C or SIGTERM.
///
/// SIGTERM is registered before this returns, so one sent while the future
/// waits to be polled is not missed.
#[cfg(unix)]
fn shutdown_requested() -> impl Future<Output = ()> + Send + 'static {
    use tokio::signal::unix::{SignalKind, signal};

    let sigterm = signal(SignalKind::terminate());

    async move {
        let mut sigterm = match sigterm {
            Ok(sigterm) => sigterm,
            Err(error) => {
                warn!(%error, "Cannot listen for SIGTERM; only Ctrl-C stops the plugin.");
                return ctrl_c().await;
            }
        };

        tokio::select! {
            () = ctrl_c() => {}
            Some(()) = sigterm.recv() => {}
        }
    }
}

/// Completes on the first Ctrl-C or Ctrl-Break.
///
/// Ctrl-Break is registered before this returns, so one sent while the future
/// waits to be polled is not missed.
#[cfg(windows)]
fn shutdown_requested() -> impl Future<Output = ()> + Send + 'static {
    use tokio::signal::windows::ctrl_break;

    let ctrl_break = ctrl_break();

    async move {
        let mut ctrl_break = match ctrl_break {
            Ok(ctrl_break) => ctrl_break,
            Err(error) => {
                warn!(%error, "Cannot listen for Ctrl-Break; only Ctrl-C stops the plugin.");
                return ctrl_c().await;
            }
        };

        tokio::select! {
            () = ctrl_c() => {}
            Some(()) = ctrl_break.recv() => {}
        }
    }
}

/// Completes on the first Ctrl-C, and never when Ctrl-C cannot be listened for.
async fn ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        warn!(%error, "Cannot listen for Ctrl-C.");
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
#[path = "plugin_tests.rs"]
mod tests;
