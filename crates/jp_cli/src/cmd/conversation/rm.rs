use jp_conversation::ConversationId;
use jp_workspace::{ConversationHandle, Workspace};

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
        time::CreationRange,
    },
    ctx::Ctx,
    shared::confirm::{ConfirmFlag, ConversationAction, confirm_conversation_action},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Rm {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Remove all conversations created in a `[--created-since,
    /// --created-before)` range.
    #[command(flatten)]
    range: CreationRange,

    /// Confirmation prompting: `--confirm`, `--no-confirm`, or `--yes`.
    ///
    /// Removal always prompts by default; `--no-confirm` / `--yes` skips it.
    /// With no user available to answer — no terminal, or `--no-interactive`
    /// — removal fails unless `--no-confirm` was passed.
    #[command(flatten)]
    confirm: ConfirmFlag,
}

impl Rm {
    pub(crate) async fn run(self, ctx: &mut Ctx, mut handles: Vec<ConversationHandle>) -> Output {
        let active_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));

        // Range mode: resolve IDs by filtering all conversations on
        // creation date. `conversation_load_request` returns `none()` in this
        // mode, so `handles` is empty here.
        if self.range.is_set() {
            handles = self.resolve_filtered(&ctx.workspace)?;

            if handles.is_empty() {
                ctx.printer.println("No conversations match the range.");
                return Ok(());
            }
        }

        // Removal is destructive, so the default (`None`) prompts; only an
        // explicit `--no-confirm` / `--yes` skips it.
        let force = self.confirm.preference() == Some(false);
        let mut removed = 0_usize;
        for handle in handles {
            if remove(ctx, handle, active_id, force).await? {
                removed += 1;
            }
        }

        if removed == 0 {
            ctx.printer.println("No conversations removed.");
        } else {
            ctx.printer.println("Conversation(s) removed.");
        }
        Ok(())
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.range.is_set() {
            ConversationLoadRequest::none()
        } else {
            ConversationLoadRequest::explicit_or_session(&self.target)
        }
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
