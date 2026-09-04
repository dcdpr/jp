use chrono::Utc;
use crossterm::style::Stylize as _;
use jp_printer::Printer;
use tracing::warn;

use crate::cmd::{
    Output,
    workspace::target::{self, ResolvedTarget, TargetEnv, WorkspaceTarget},
};

/// Select the session's active workspace.
///
/// After `jp w use`, workspace-consuming commands run against the selection
/// from anywhere, the way an active conversation follows the session (RFD 020).
/// `jp w use ?` opens a picker; `jp w use cwd` drops the selection and returns
/// to cwd resolution.
///
/// Interactive-only in every form — including `cwd` — because it mutates
/// session state; scripts target a workspace per invocation with `jp
/// --workspace` instead.
#[derive(Debug, clap::Args)]
pub(crate) struct Use {
    /// The workspace to select.
    /// See `jp w use help` for the grammar.
    ///
    /// Also settable with the global `--workspace` flag, but not both at once.
    /// Defaults to the picker (`?`).
    pub(super) target: Option<WorkspaceTarget>,

    /// Keep using this workspace even from inside another one.
    ///
    /// Without it, standing in a different workspace makes JP ask which of the
    /// two you meant.
    /// With it, the selection wins and JP stops asking; every run from
    /// elsewhere reports the workspace it used.
    ///
    /// A later `jp workspace use` without `--always` releases it, and `jp
    /// workspace use cwd` clears the selection outright.
    #[arg(long)]
    pub(super) always: bool,
}

impl Use {
    pub(crate) fn run(self, printer: &Printer, env: &TargetEnv<'_>) -> Output {
        let target = self.target.unwrap_or(WorkspaceTarget::Picker);

        if matches!(target, WorkspaceTarget::Help) {
            printer.println(target::help());
            return Ok(());
        }

        // Interactive-only: the selection is hidden per-session state, and a
        // script that mutated it would stop being deterministic. Scripts
        // return to cwd behavior by not setting $JP_SESSION, not by running
        // `jp w use cwd` (RFD 087).
        if !env.interactive {
            return Err(format!(
                "`jp workspace use` is interactive-only. Scripts target a workspace per \
                 invocation with `{}` instead.",
                "--workspace <id|path>".bold().yellow(),
            )
            .into());
        }

        let Some(session) = env.session else {
            return Err(
                "No session identity available. Set $JP_SESSION or run in a terminal with \
                 automatic session detection."
                    .into(),
            );
        };

        // `cwd` drops the record the sticky flag would live on, so the two
        // together ask for a selection that is both absent and permanent.
        if self.always && matches!(target, WorkspaceTarget::Cwd) {
            return Err(format!(
                "`{}` clears the session-active workspace, so there is nothing for `{}` to keep \
                 active.",
                "jp workspace use cwd".bold().yellow(),
                "--always".bold().yellow(),
            )
            .into());
        }

        let mapping = env.store.load(session);
        let was_sticky = mapping.as_ref().is_some_and(|mapping| mapping.sticky);
        let previous = mapping.and_then(|mapping| mapping.history.into_iter().next());
        let suffix = sticky_suffix(self.always, was_sticky);

        match target::resolve(&target, env)? {
            ResolvedTarget::Help => unreachable!("handled before resolution"),

            // Clearing is just selecting the cwd-derived workspace: the
            // record — history and sticky flag included — is dropped, and
            // resolution falls back to the directory the command runs from.
            ResolvedTarget::Cwd => {
                env.store.clear(session)?;

                match previous {
                    Some(entry) => printer.println(format!(
                        "Cleared the session-active workspace ({}); falling back to cwd \
                         resolution.",
                        entry.root.to_string().bold().grey(),
                    )),
                    None => printer.println(
                        "No session-active workspace was set; using cwd resolution.".to_owned(),
                    ),
                }
            }

            ResolvedTarget::Root(selected) => {
                let Some(id) = selected.id else {
                    return Err(format!(
                        "`{}` is not a recognizable JP workspace: its `{}` ID file is missing or \
                         unreadable.",
                        selected.root,
                        crate::DEFAULT_STORAGE_DIR,
                    )
                    .into());
                };

                // Announce the checkout before recording the selection: a
                // later run resolves this selection from anywhere by ID, which
                // only works once the roots registry knows the checkout. A
                // freshly cloned workspace no command has run inside yet is
                // exactly the case `use` has to cover.
                if let Err(error) =
                    crate::register_workspace_checkout(&env.workspaces_dir, &selected.root, &id)
                {
                    warn!(%error, root = %selected.root, "Failed to register the workspace checkout.");
                }

                if previous.as_ref().is_some_and(|entry| {
                    entry.id().is_some_and(|prev| prev == id) && entry.root == selected.root
                }) {
                    // Re-selecting the same workspace still restates the flag:
                    // `--always` on an already-active workspace is how a user
                    // keeps the one they are standing in.
                    env.store.set_sticky(session, self.always)?;

                    printer.println(format!(
                        "Already the session-active workspace: {}{suffix}",
                        selected.root.to_string().bold().yellow(),
                    ));
                    return Ok(());
                }

                // The invocation states the whole intent for the session: a
                // selection made without `--always` is one made without the
                // flag, including when an earlier selection left one behind.
                env.store.record_selection_with_sticky(
                    session,
                    &id,
                    &selected.root,
                    Utc::now(),
                    self.always,
                )?;

                let to = selected.root.to_string().bold().yellow();
                match previous {
                    Some(entry) => printer.println(format!(
                        "Switched the session-active workspace from {} to {to}{suffix}",
                        entry.root.to_string().bold().grey(),
                    )),
                    None => {
                        printer.println(format!("Session-active workspace set to {to}{suffix}"));
                    }
                }
            }
        }

        Ok(())
    }
}

/// The trailing clause naming what the invocation did to the session's sticky
/// flag, empty when it leaves the flag off.
fn sticky_suffix(always: bool, was_sticky: bool) -> &'static str {
    match (always, was_sticky) {
        (true, _) => " (always, ignoring the current directory)",
        (false, true) => " (no longer always; the current directory applies again)",
        (false, false) => "",
    }
}

#[cfg(test)]
#[path = "use_tests.rs"]
mod tests;
