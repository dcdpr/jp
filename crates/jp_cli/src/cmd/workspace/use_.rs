use chrono::Utc;
use crossterm::style::Stylize as _;
use jp_printer::Printer;

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
    /// Defaults to the picker (`?`).
    target: Option<WorkspaceTarget>,
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

                if previous.as_ref().is_some_and(|entry| {
                    entry.id().is_some_and(|prev| prev == id) && entry.root == selected.root
                }) {
                    printer.println(format!(
                        "Already the session-active workspace: {}",
                        selected.root.to_string().bold().yellow(),
                    ));
                    return Ok(());
                }

                env.store
                    .record_selection(session, &id, &selected.root, Utc::now())?;

                let to = selected.root.to_string().bold().yellow();
                match previous {
                    Some(entry) => printer.println(format!(
                        "Switched the session-active workspace from {} to {to}",
                        entry.root.to_string().bold().grey(),
                    )),
                    None => printer.println(format!("Session-active workspace set to {to}")),
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "use_tests.rs"]
mod tests;
