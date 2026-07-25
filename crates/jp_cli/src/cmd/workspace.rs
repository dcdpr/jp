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
use target::TargetEnv;

use crate::{bootstrap::WorkspaceRequirement, cmd::Output};

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
    pub(crate) fn run(self, printer: &Printer, session: Option<&Session>, persist: bool) -> Output {
        let env = TargetEnv::new(session)?;

        match self.command {
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
