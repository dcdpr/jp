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
    /// A later `jp workspace use` without `--always` releases the pin, and `jp
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

        // `cwd` drops the record the pin would live on, so the two together
        // ask for a selection that is both absent and permanent.
        if self.always && matches!(target, WorkspaceTarget::Cwd) {
            return Err(format!(
                "`{}` clears the session-active workspace, so there is nothing for `{}` to pin to.",
                "jp workspace use cwd".bold().yellow(),
                "--always".bold().yellow(),
            )
            .into());
        }

        let previous = env.store.active(session);

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
                    // Re-selecting the same workspace still restates the pin:
                    // `--always` on an already-active workspace is how a user
                    // pins one they are standing in.
                    env.store.set_sticky(session, self.always)?;

                    printer.println(format!(
                        "Already the session-active workspace: {}{}",
                        selected.root.to_string().bold().yellow(),
                        pin_suffix(self.always),
                    ));
                    return Ok(());
                }

                env.store
                    .record_selection(session, &id, &selected.root, Utc::now())?;

                // The invocation states the whole intent for the session: a
                // selection made without `--always` is one made without a pin,
                // including when an earlier selection left one behind.
                env.store.set_sticky(session, self.always)?;

                let to = selected.root.to_string().bold().yellow();
                let pinned = pin_suffix(self.always);
                match previous {
                    Some(entry) => printer.println(format!(
                        "Switched the session-active workspace from {} to {to}{pinned}",
                        entry.root.to_string().bold().grey(),
                    )),
                    None => {
                        printer.println(format!("Session-active workspace set to {to}{pinned}"));
                    }
                }
            }
        }

        Ok(())
    }
}

/// The trailing clause naming the pin, for a selection that has one.
fn pin_suffix(always: bool) -> &'static str {
    if always {
        " (always, ignoring the current directory)"
    } else {
        ""
    }
}

#[cfg(test)]
#[path = "use_tests.rs"]
mod tests;
