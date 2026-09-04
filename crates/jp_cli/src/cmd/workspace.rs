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

use jp_printer::Printer;
use jp_workspace::session::Session;
use target::{TargetEnv, WorkspaceTarget};

use crate::{
    bootstrap::WorkspaceRequirement,
    cmd::{self, Output},
};

/// Manage workspaces.
#[derive(Debug, clap::Args)]
pub(crate) struct Workspace {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Select the session's active workspace.
    #[command(name = "use", visible_alias = "u")]
    Use(use_::Use),

    /// List known workspaces and their checkouts.
    #[command(name = "ls", alias = "list")]
    Ls(ls::Ls),

    /// Show a workspace: identity, checkouts, and how it resolves.
    #[command(name = "show", visible_alias = "s")]
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

    /// What each subcommand needs from the workspace bootstrap (RFD 087).
    ///
    /// `ls` reads the user-global registries only; `use` resolves and validates
    /// a target root to record a selection; `show` additionally loads
    /// conversation indexes for its count.
    pub(crate) fn workspace_requirement(&self) -> WorkspaceRequirement {
        match &self.command {
            Commands::Ls(_) => WorkspaceRequirement::None,
            Commands::Use(_) => WorkspaceRequirement::Resolve,
            Commands::Show(_) => WorkspaceRequirement::Load,
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
