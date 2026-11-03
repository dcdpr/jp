//! The `jp workspace` (`jp w`) command surface (RFD 087).
//!
//! Mirrors `jp conversation` one level up: `use` selects the session's active
//! workspace, `ls` lists known workspaces, `show` reports one.
//!
//! These commands run on a dedicated pre-workspace path: selecting or
//! inspecting a workspace must work from outside every workspace — including
//! resolving to *no* workspace — so they receive the pre-workspace
//! [`TargetEnv`] instead of a `Ctx`, and never construct a
//! [`jp_workspace::Workspace`] except where their own semantics load one
//! (`show`'s conversation count).

mod ls;
mod show;
pub(crate) mod target;
mod use_;

use clap::{ArgMatches, Command};
use jp_printer::Printer;
use jp_workspace::session::Session;
use target::{TargetEnv, WorkspaceTarget};

use crate::{
    cli::{CommandArgs, subcommand_of, unread_subcommand},
    cmd::{self, Output},
};

/// `jp workspace`: manage workspaces.
#[derive(Debug)]
pub(crate) struct Workspace {
    command: Commands,
}

impl CommandArgs for Workspace {
    fn augment(cmd: Command) -> Command {
        cmd.subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                use_::Use::augment(Command::new("use"))
                    .about("Select the session's active workspace")
                    .visible_alias("u"),
            )
            .subcommand(
                ls::Ls::augment(Command::new("ls"))
                    .about("List known workspaces and their checkouts")
                    .alias("list"),
            )
            .subcommand(
                show::Show::augment(Command::new("show"))
                    .about("Show a workspace: identity, checkouts, and how it resolves")
                    .visible_alias("s"),
            )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let command = match subcommand_of(matches)? {
            ("use", sub) => Commands::Use(use_::Use::from_matches(sub)?),
            ("ls", sub) => Commands::Ls(ls::Ls::from_matches(sub)?),
            ("show", sub) => Commands::Show(show::Show::from_matches(sub)?),
            (name, _) => return Err(unread_subcommand(name)),
        };

        Ok(Self { command })
    }
}

#[derive(Debug)]
enum Commands {
    Use(use_::Use),
    Ls(ls::Ls),
    Show(show::Show),
}

impl Workspace {
    /// Run the subcommand.
    ///
    /// `global` is the invocation-wide `--workspace` target.
    /// It names the workspace `use` and `show` act on, so `jp -w foo w show` is
    /// a spelling of `jp w show foo`.
    pub(crate) fn run(
        self,
        printer: &Printer,
        session: Option<&Session>,
        persist: bool,
        global: Option<&WorkspaceTarget>,
        no_interactive: bool,
    ) -> Output {
        // Reconcile arguments before resolving the environment, so a
        // contradictory invocation fails on its own terms rather than on
        // whatever the user data directory happens to hold.
        let command = self.command.with_global_target(global)?;
        let env = TargetEnv::new(session, no_interactive)?;

        match command {
            Commands::Use(args) => args.run(printer, &env),
            Commands::Ls(args) => args.run(printer, &env),
            Commands::Show(args) => args.run(printer, &env, persist),
        }
    }
}

impl Commands {
    /// Fold the global `--workspace` target into the subcommand's own.
    ///
    /// `use` and `show` take a workspace, so the flag is an alternative
    /// spelling of their positional argument.
    /// `ls` takes none, so the flag can only be a mistake there.
    fn with_global_target(mut self, global: Option<&WorkspaceTarget>) -> Result<Self, cmd::Error> {
        match &mut self {
            Commands::Use(args) => {
                args.target = target_for("jp w use", args.target.take(), global)?;
            }
            Commands::Show(args) => {
                args.target = target_for("jp w show", args.target.take(), global)?;
            }
            Commands::Ls(_) if global.is_some() => {
                return Err(cmd::Error::from(
                    "`jp w ls` lists every known workspace, so the global `--workspace` flag has \
                     nothing to select. Drop the flag, or inspect one workspace with `jp w show \
                     <target>`.",
                ));
            }
            Commands::Ls(_) => {}
        }

        Ok(self)
    }
}

/// The target a subcommand acts on, given its own argument and the global
/// `--workspace` flag.
///
/// The two are spellings of the same thing, so naming a workspace twice is
/// rejected rather than resolved by precedence: a silently ignored target is
/// the failure mode this reconciliation exists to avoid.
fn target_for(
    command: &str,
    positional: Option<WorkspaceTarget>,
    global: Option<&WorkspaceTarget>,
) -> Result<Option<WorkspaceTarget>, cmd::Error> {
    match (positional, global) {
        (Some(_), Some(_)) => Err(cmd::Error::from(format!(
            "`{command}` was given a target and the global `--workspace` flag. Name the workspace \
             once."
        ))),
        (Some(target), None) => Ok(Some(target)),
        (None, Some(target)) => Ok(Some(target.clone())),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
