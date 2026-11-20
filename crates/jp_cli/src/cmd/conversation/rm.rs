use std::sync::Arc;

use camino::Utf8PathBuf;
use clap::{ArgMatches, Command};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_storage::{
    backend::{FsStorageBackend, LockBackend as _, PersistBackend as _},
    validate::{DirContents, stored_title, validate_conversation_dir},
};
use jp_workspace::{ConversationHandle, Error as WorkspaceError, Workspace};

use crate::{
    cli::CommandArgs,
    cmd::{
        self, ConversationLoadRequest, Output,
        conversation_id::{ConversationIds as _, PositionalIds},
        lock::{LockOutcome, LockRequest, acquire_lock},
        target::ConversationTarget,
        time::CreationRange,
    },
    ctx::Ctx,
    shared::confirm::{
        ConfirmFlag, ConversationAction, confirm_conversation_action, confirm_unloaded_removal,
    },
};

/// `jp conversation rm`.
#[derive(Debug)]
pub(crate) struct Rm {
    target: PositionalIds<true, true>,

    /// Remove all conversations created in a `[--created-since,
    /// --created-before)` range.
    range: CreationRange,

    /// Confirmation prompting: `--confirm`, `--no-confirm`, or `--yes`.
    ///
    /// Removal always prompts by default; `--no-confirm` / `--yes` skips it.
    /// With no user available to answer — no terminal, or `--no-interactive`
    /// — removal fails unless `--no-confirm` was passed.
    confirm: ConfirmFlag,
}

impl CommandArgs for Rm {
    fn augment(cmd: Command) -> Command {
        let cmd = PositionalIds::<true, true>::augment(cmd);
        let cmd = CreationRange::<true>::augment(cmd);
        ConfirmFlag::augment(cmd)
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            target: PositionalIds::from_matches(matches)?,
            range: CreationRange::from_matches(matches)?,
            confirm: ConfirmFlag::from_matches(matches)?,
        })
    }
}

impl Rm {
    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let active_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));

        // `conversation_load_request` returns `none()` in range mode and for
        // literal IDs, so `handles` is empty in both and the targets are
        // resolved here.
        let removals = if self.range.is_set() {
            let handles = self.resolve_filtered(&ctx.workspace)?;
            if handles.is_empty() {
                ctx.printer.println("No conversations match the range.");
                return Ok(());
            }
            handles.into_iter().map(Removal::Loaded).collect()
        } else if let Some(ids) = self.literal_ids() {
            resolve_literal(ctx, &ids)?
        } else {
            handles.into_iter().map(Removal::Loaded).collect::<Vec<_>>()
        };

        // Removal is destructive, so the default (`None`) prompts; only an
        // explicit `--no-confirm` / `--yes` skips it.
        let force = self.confirm.preference() == Some(false);
        let mut removed = 0_usize;
        for removal in removals {
            let done = match removal {
                Removal::Loaded(handle) => remove(ctx, handle, active_id, force).await?,
                Removal::Unloaded { fs, id } => remove_unloaded(ctx, &fs, id, force)?,
            };
            if done {
                removed += 1;
            }
        }

        if removed == 0 {
            ctx.printer.println("No conversations removed.");
        } else {
            ctx.printer.println("Conversation(s) removed.");
        }
        if !ctx.term.args.persist {
            ctx.printer.println("");
            ctx.printer.println(format!(
                "This command ran under `--no-persist`, {}.",
                "no files on disk were modified or removed".bold()
            ));
        }
        Ok(())
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.range.is_set() || self.literal_ids().is_some() {
            ConversationLoadRequest::none()
        } else {
            ConversationLoadRequest::explicit_or_session(&self.target)
        }
    }

    /// The targets, when every one is a typed conversation ID.
    ///
    /// Those are resolved by `run` rather than through the conversation index,
    /// which leaves out conversations that fail to load.
    fn literal_ids(&self) -> Option<Vec<ConversationId>> {
        let targets = self.target.ids();
        if targets.is_empty() {
            return None;
        }

        targets
            .iter()
            .map(|target| match target {
                ConversationTarget::Id(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// Resolve handles by applying the range filter over the workspace.
    ///
    /// Extracted from `run` so the filter step can be exercised in tests
    /// without driving the async confirmation/lock path.
    /// This is the dividing line between `--created-since`/`--created-before`
    /// and actual deletion; regressing it would silently turn a range delete
    /// into a full wipe.
    fn resolve_filtered(
        &self,
        workspace: &Workspace,
    ) -> Result<Vec<ConversationHandle>, crate::error::Error> {
        workspace
            .conversations()
            .filter(|(id, _)| self.range.matches(**id))
            .map(|(id, _)| workspace.acquire_conversation(id).map_err(Into::into))
            .collect()
    }
}

/// A conversation `rm` is about to remove.
enum Removal {
    /// A conversation in the conversation index.
    Loaded(ConversationHandle),

    /// A conversation the index leaves out, because it fails to load or was
    /// never stored, that had live directories when it was resolved.
    Unloaded {
        fs: Arc<FsStorageBackend>,
        id: ConversationId,
    },
}

/// Resolve typed IDs: through the index when the conversation loads, otherwise
/// by its live directories on disk.
///
/// Every ID is resolved before anything is removed, so one unknown ID fails the
/// run without touching the others.
fn resolve_literal(ctx: &Ctx, ids: &[ConversationId]) -> Result<Vec<Removal>, cmd::Error> {
    ids.iter()
        .map(|id| {
            let not_found = match ctx.workspace.acquire_conversation(id) {
                Ok(handle) => return Ok(Removal::Loaded(handle)),
                Err(error) => error,
            };

            let unloaded = ctx
                .fs_backend
                .as_ref()
                .filter(|fs| !fs.live_conversation_dirs(id).is_empty())
                .map(|fs| Removal::Unloaded {
                    fs: Arc::clone(fs),
                    id: *id,
                });
            unloaded.ok_or_else(|| crate::error::Error::from(not_found).into())
        })
        .collect()
}

/// Remove a conversation the index leaves out, asking first unless `force`.
///
/// Its live directories, title, and status are read while its lock is held, so
/// the prompt describes exactly what the removal deletes.
/// Under `--no-persist` it checks the lock without taking it and deletes
/// nothing.
/// Returns whether the conversation was (or, in a dry run, would be) removed.
fn remove_unloaded(
    ctx: &Ctx,
    fs: &FsStorageBackend,
    id: ConversationId,
    force: bool,
) -> Result<bool, cmd::Error> {
    let dry_run = !ctx.term.args.persist;
    let locked = || cmd::Error::from(format!("Conversation {id} is in use by another process."));

    let _guard = if dry_run {
        if fs.is_conversation_locked(&id) {
            return Err(locked());
        }
        None
    } else {
        Some(fs.try_lock(&id.to_string(), None)?.ok_or_else(locked)?)
    };

    let dirs = fs.live_conversation_dirs(&id);
    if dirs.is_empty() {
        // Removed by another process since it was resolved.
        return Err(
            crate::error::Error::from(WorkspaceError::not_found("Conversation", &id)).into(),
        );
    }

    if !force {
        let title = dirs.iter().find_map(|dir| stored_title(dir));
        let status = unloaded_status(&dirs);
        if !confirm_unloaded_removal(ctx, id, title.as_deref(), status.as_deref(), &dirs)? {
            return Ok(false);
        }
    }

    if !dry_run {
        fs.remove(&id)?;
    }
    Ok(true)
}

/// Why a conversation does not load, for the removal prompt, or `None` when it
/// does.
///
/// A conversation the index leaves out can load by the time it is removed, when
/// another process finished writing it after this run's scan.
fn unloaded_status(dirs: &[Utf8PathBuf]) -> Option<String> {
    let mut stored = false;
    for dir in dirs {
        match validate_conversation_dir(dir) {
            Ok(DirContents::Stored) => stored = true,
            Ok(DirContents::Unstored) => {}
            Err(error) => return Some(format!("unable to load: {error}")),
        }
    }

    (!stored).then(|| "never stored".to_owned())
}

/// Remove one conversation, asking first unless `force` says the caller already
/// decided.
///
/// Returns whether the conversation was removed.
/// Declining leaves it in place and is not an error: the user answered the
/// question they were asked, and the next conversation in the run still gets
/// its own.
///
/// The lock is taken before the question is asked and held until it is
/// answered, so the conversation cannot gain events between the details the
/// user read and the removal they approved.
async fn remove(
    ctx: &mut Ctx,
    handle: ConversationHandle,
    active_id: Option<ConversationId>,
    force: bool,
) -> Result<bool, crate::error::Error> {
    let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
        LockOutcome::Acquired(lock) => lock,
        LockOutcome::NewConversation => unreachable!("new conversation not allowed"),
        LockOutcome::ForkConversation(_) => unreachable!("fork not allowed"),
    };

    if !force && !confirm_conversation_action(ctx, ConversationAction::Remove, &lock, active_id)? {
        return Ok(false);
    }

    ctx.workspace.remove_conversation_with_lock(lock.into_mut());
    Ok(true)
}

#[cfg(test)]
#[path = "rm_tests.rs"]
mod tests;
