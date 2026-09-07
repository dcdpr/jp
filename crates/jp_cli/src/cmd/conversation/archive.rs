use crossterm::style::Stylize as _;
use jp_conversation::{Conversation, ConversationId};
use jp_workspace::{ConversationHandle, Workspace};

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
        time::{CreationRange, TimeThreshold},
    },
    ctx::Ctx,
    shared::confirm::{ConfirmFlag, ConversationAction, confirm_conversation_action},
};

/// Archive conversations.
///
/// Without IDs, archives the session's active conversation (same fallback chain
/// as `jp c show`: session active → `conversation.default_id` → picker).
/// With IDs, archives each one.
///
/// By default, prompts for confirmation when archiving pinned or active
/// conversations, or when archiving more than one conversation at once.
/// Pass `--confirm` to prompt for every conversation, or `--no-confirm` /
/// `--yes` to skip all prompts.
/// With no user available to answer — no terminal, or `--no-interactive` — a
/// required confirmation fails rather than skipping the conversation.
///
/// Use `--created-since`/`--created-before` to archive a range of conversations
/// by creation date, or `--inactive-since` to archive everything unused since a
/// given time.
/// The three filters AND together when combined.
///
/// Archived conversations are hidden from listings and pickers.
/// Use `jp c ls --archived` to list them, `jp c unarchive` to restore them, or
/// `jp c use archived` to unarchive and activate.
#[derive(Debug, clap::Args)]
pub(crate) struct Archive {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Archive all conversations created in a `[--created-since,
    /// --created-before)` range.
    #[command(flatten)]
    range: CreationRange,

    /// Archive all conversations inactive since a given time.
    ///
    /// Accepts the same formats as `--created-since`.
    /// Filters on `last_activated_at` (when the conversation was last used)
    /// rather than its creation date, which makes this distinct from the
    /// `--created-since`/`--created-before` range.
    #[arg(long, conflicts_with = "id")]
    inactive_since: Option<TimeThreshold>,

    /// Confirmation prompting: `--confirm`, `--no-confirm`, or `--yes`.
    ///
    /// Without a flag, prompts only for pinned or active conversations, or when
    /// archiving more than one conversation at once.
    #[command(flatten)]
    confirm: ConfirmFlag,
}

impl Archive {
    /// Whether any of the filter flags is set.
    fn has_filter(&self) -> bool {
        self.range.is_set() || self.inactive_since.is_some()
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.has_filter() {
            // Filter mode resolves conversations internally.
            return ConversationLoadRequest::none();
        }

        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let handles = if self.has_filter() {
            let filtered = self.resolve_filtered(&ctx.workspace)?;
            if filtered.is_empty() {
                ctx.printer.println("No conversations match the filter.");
                return Ok(());
            }
            filtered
        } else {
            handles
        };

        let active_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));
        let preference = self.confirm.preference();
        let multi = handles.len() > 1;

        for handle in handles {
            let id = handle.id();

            // The lock comes before the question and is held until it is
            // answered, so a conversation another tab is still writing to
            // cannot be archived out from under it by an answer given an hour
            // after the details were read.
            let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
                LockOutcome::Acquired(lock) => lock,
                LockOutcome::NewConversation | LockOutcome::ForkConversation(_) => unreachable!(),
            };

            let asks = needs_confirmation(
                preference,
                active_id == Some(id),
                lock.metadata().is_pinned(),
                multi,
            );

            if asks
                && !confirm_conversation_action(ctx, ConversationAction::Archive, &lock, active_id)?
            {
                continue;
            }

            ctx.workspace.archive_conversation(lock.into_mut())?;
            ctx.printer.println(format!(
                "Conversation {} archived.",
                id.to_string().bold().yellow()
            ));
        }

        Ok(())
    }

    /// AND-composition of the active filter flags.
    /// Pure for testability.
    fn matches(&self, id: ConversationId, conv: &Conversation) -> bool {
        self.range.matches(id)
            && self
                .inactive_since
                .is_none_or(|t| conv.last_activated_at < *t)
    }

    /// Resolve handles by applying `matches` over the workspace.
    fn resolve_filtered(
        &self,
        workspace: &Workspace,
    ) -> Result<Vec<ConversationHandle>, crate::error::Error> {
        workspace
            .conversations()
            .filter(|(id, c)| self.matches(**id, c))
            .map(|(id, _)| workspace.acquire_conversation(id).map_err(Into::into))
            .collect()
    }
}

/// Whether archiving a conversation asks the user first.
///
/// `preference` is the resolved `--confirm` / `--no-confirm` choice and wins
/// outright when set.
/// Without one, a pinned or session-active conversation asks, and so does every
/// conversation in a run covering more than one (`multi`).
const fn needs_confirmation(
    preference: Option<bool>,
    is_active: bool,
    is_pinned: bool,
    multi: bool,
) -> bool {
    match preference {
        Some(explicit) => explicit,
        None => is_active || is_pinned || multi,
    }
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
