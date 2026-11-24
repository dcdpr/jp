use clap::{ArgMatches, Command};
use jp_conversation::ConversationId;
use jp_storage::backend::Removal as StorageRemoval;
use jp_workspace::{ConversationHandle, Workspace};

use crate::{
    cli::CommandArgs,
    cmd::{
        self, ConversationLoadRequest, Output,
        conversation_id::{ConversationIds as _, PositionalIds},
        lock::{LockOutcome, LockRequest, acquire_lock, wait_until_free},
        target::ConversationTarget,
        time::CreationRange,
    },
    ctx::Ctx,
    format::no_persist_note,
    shared::confirm::{Answers, ConfirmFlag, ConversationAction, confirm_unloaded_removal},
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
        let mut answers = self.confirm.answers();
        let count = removals.len();
        let mut removed = 0_usize;
        for (index, removal) in removals.into_iter().enumerate() {
            // `N` declined the rest, so none of their locks is taken.
            if answers.rest() == Some(false) {
                break;
            }

            let more = index + 1 < count;
            let done = match removal {
                Removal::Loaded(handle) => {
                    remove(ctx, handle, active_id, &mut answers, more).await?
                }
                Removal::Unloaded(id) => remove_unloaded(ctx, id, &mut answers, more).await?,
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
        if !ctx.workspace.persists() {
            ctx.printer.println("");
            ctx.printer.println(no_persist_note());
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
    /// never stored.
    Unloaded(ConversationId),
}

/// Resolve typed IDs: through the index when the conversation loads, otherwise
/// as a stored conversation the index leaves out.
///
/// Every ID is resolved before anything is removed, so one unknown ID fails the
/// run without touching the others.
fn resolve_literal(ctx: &Ctx, ids: &[ConversationId]) -> Result<Vec<Removal>, cmd::Error> {
    ids.iter()
        .map(|id| match ctx.workspace.acquire_conversation(id) {
            Ok(handle) => Ok(Removal::Loaded(handle)),
            Err(_) if ctx.recovery.inspect(id).is_some() => Ok(Removal::Unloaded(*id)),
            Err(not_found) => Err(crate::error::Error::from(not_found).into()),
        })
        .collect()
}

/// Remove a conversation the index leaves out, asking first unless `answers`
/// already holds the answer.
///
/// What the prompt shows is read while the conversation's lock is held, so it
/// describes exactly what the removal deletes.
/// While another process holds the lock, waits as for any other conversation.
/// `more` says whether more conversations follow this one.
/// Returns whether the conversation was removed.
async fn remove_unloaded(
    ctx: &Ctx,
    id: ConversationId,
    answers: &mut Answers,
    more: bool,
) -> Result<bool, cmd::Error> {
    loop {
        // `decide` can only answer yes or no, so a failed prompt answers no and
        // its error is returned once the lock is released.
        let mut prompt_error = None;
        let removal = ctx.recovery.remove(&id, &mut |stored| {
            answers
                .decide(more, |more| confirm_unloaded_removal(ctx, stored, more))
                .unwrap_or_else(|error| {
                    prompt_error = Some(error);
                    false
                })
        })?;

        if let Some(error) = prompt_error {
            return Err(error.into());
        }

        match removal {
            StorageRemoval::Locked => wait_until_free(ctx, id).await?,
            StorageRemoval::Removed => return Ok(true),
            StorageRemoval::Declined => return Ok(false),
            StorageRemoval::NotFound => {
                return Err(crate::error::Error::from(jp_workspace::Error::not_found(
                    "Conversation",
                    &id,
                ))
                .into());
            }
        }
    }
}

/// Remove one conversation, asking first unless `answers` already holds the
/// answer.
///
/// `more` says whether more conversations follow this one.
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
    answers: &mut Answers,
    more: bool,
) -> Result<bool, crate::error::Error> {
    let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
        LockOutcome::Acquired(lock) => lock,
        LockOutcome::NewConversation => unreachable!("new conversation not allowed"),
        LockOutcome::ForkConversation(_) => unreachable!("fork not allowed"),
    };

    if !answers.confirm_action(ctx, ConversationAction::Remove, &lock, active_id, more)? {
        return Ok(false);
    }

    ctx.workspace.remove_conversation_with_lock(lock.into_mut());
    Ok(true)
}

#[cfg(test)]
#[path = "rm_tests.rs"]
mod tests;
