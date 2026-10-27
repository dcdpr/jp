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

use clap::{Arg, ArgMatches, Command};
use jp_printer::Printer;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::{
    KeyValueOrPath,
    cli::{CommandArgs, subcommand_of, unread_subcommand},
    cmd,
};

/// `jp plugin` subcommand group for managing plugins.
#[derive(Debug)]
pub(crate) struct PluginManagement {
    command: PluginCmd,
}

impl CommandArgs for PluginManagement {
    fn augment(cmd: Command) -> Command {
        cmd.subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                list::List::augment(Command::new("list"))
                    .about("List installed plugins, and the ones the registry offers")
                    .visible_alias("ls"),
            )
            .subcommand(
                install::Install::augment(Command::new("install"))
                    .about("Install a plugin from the registry, with the plugins it requires"),
            )
            .subcommand(
                uninstall::Uninstall::augment(Command::new("uninstall"))
                    .about("Remove a plugin JP installed, and its approval"),
            )
            .subcommand(
                update::Update::augment(Command::new("update"))
                    .about("Refresh the plugin registry, and update the plugins JP installed"),
            )
            .subcommand(
                approve::Approve::augment(Command::new("approve"))
                    .about("Approve a plugin binary, so it runs without asking"),
            )
            .subcommand(
                revoke::Revoke::augment(Command::new("revoke"))
                    .about("Withdraw a plugin's approval, so it asks again before it runs"),
            )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let command = match subcommand_of(matches)? {
            ("list", sub) => PluginCmd::List(list::List::from_matches(sub)?),
            ("install", sub) => PluginCmd::Install(install::Install::from_matches(sub)?),
            ("uninstall", sub) => PluginCmd::Uninstall(uninstall::Uninstall::from_matches(sub)?),
            ("update", sub) => PluginCmd::Update(update::Update::from_matches(sub)?),
            ("approve", sub) => PluginCmd::Approve(approve::Approve::from_matches(sub)?),
            ("revoke", sub) => PluginCmd::Revoke(revoke::Revoke::from_matches(sub)?),
            (name, _) => return Err(unread_subcommand(name)),
        };

        Ok(Self { command })
    }
}

#[derive(Debug)]
enum PluginCmd {
    List(list::List),
    Install(install::Install),
    Uninstall(uninstall::Uninstall),
    Update(update::Update),
    Approve(approve::Approve),
    Revoke(revoke::Revoke),
}

/// The required positional `NAME` naming one plugin, as its file name without
/// `jp-`, described by `help`.
fn plugin_name_arg(help: &'static str) -> Arg {
    Arg::new("name")
        .required(true)
        .value_name("NAME")
        .help(help)
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
