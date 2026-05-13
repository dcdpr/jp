use crossterm::style::Stylize as _;
use jp_conversation::{Conversation, ConversationId};
use jp_inquire::InlineOption;
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
        time::{CreationRange, TimeThreshold},
    },
    ctx::Ctx,
};

/// Archive conversations.
///
/// Without IDs, archives the session's active conversation (same fallback
/// chain as `jp c show`: session active → `conversation.default_id` →
/// picker). With IDs, archives each one. Prompts for confirmation when
/// archiving pinned or active conversations (suppress with `--yes`).
///
/// Use `--from`/`--until` to archive a range of conversations by creation
/// date, or `--inactive-since` to archive everything unused since a given
/// time. The three filters AND together when combined.
///
/// Archived conversations are hidden from listings and pickers. Use `jp c ls
/// --archived` to list them, `jp c unarchive` to restore them, or `jp c use
/// archived` to unarchive and activate.
#[derive(Debug, clap::Args)]
pub(crate) struct Archive {
    #[command(flatten)]
    target: PositionalIds<false, true>,

    /// Archive all conversations created in a `[--from, --until)` range.
    #[command(flatten)]
    range: CreationRange,

    /// Archive all conversations inactive since a given time.
    ///
    /// Accepts the same formats as `--from`. Filters on `last_activated_at`
    /// (when the conversation was last used) rather than its creation date,
    /// which makes this distinct from `--until`.
    #[arg(long, conflicts_with = "id")]
    inactive_since: Option<TimeThreshold>,

    /// Do not prompt for confirmation on pinned or active conversations.
    #[arg(long, short = 'y')]
    yes: bool,
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
            let filtered = self.resolve_filtered(ctx)?;
            if filtered.is_empty() {
                ctx.printer.println("No conversations match the filter.");
                return Ok(());
            }
            filtered
        } else {
            handles
        };

        for handle in handles {
            let id = handle.id();

            if !self.yes && !confirm_archive(ctx, &id)? {
                continue;
            }

            let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
                LockOutcome::Acquired(lock) => lock,
                LockOutcome::NewConversation | LockOutcome::ForkConversation(_) => unreachable!(),
            };
            ctx.workspace.archive_conversation(lock.into_mut());
            ctx.printer.println(format!(
                "Conversation {} archived.",
                id.to_string().bold().yellow()
            ));
        }

        Ok(())
    }

    /// AND-composition of the active filter flags. Pure for testability.
    fn matches(&self, id: ConversationId, conv: &Conversation) -> bool {
        self.range.matches(id)
            && self
                .inactive_since
                .is_none_or(|t| conv.last_activated_at < *t)
    }

    /// Resolve handles by applying `matches` over the workspace.
    fn resolve_filtered(&self, ctx: &Ctx) -> Result<Vec<ConversationHandle>, crate::error::Error> {
        ctx.workspace
            .conversations()
            .filter(|(id, c)| self.matches(**id, c))
            .map(|(id, _)| ctx.workspace.acquire_conversation(id).map_err(Into::into))
            .collect()
    }
}

/// Prompt for confirmation when archiving a pinned or active conversation.
///
/// Returns `true` if the user confirms (or no prompt is needed).
fn confirm_archive(ctx: &mut Ctx, id: &ConversationId) -> Result<bool, crate::error::Error> {
    let handle = ctx.workspace.acquire_conversation(id)?;
    let meta = ctx.workspace.metadata(&handle)?;

    let is_active = ctx
        .session
        .as_ref()
        .and_then(|s| ctx.workspace.session_active_conversation(s))
        == Some(*id);
    let is_pinned = meta.is_pinned();

    if !is_active && !is_pinned {
        return Ok(true);
    }

    // Active subsumes pinned in the prompt.
    let qualifier = if is_active { "active" } else { "pinned" };
    let prompt = format!(
        "Archive the {qualifier} conversation {}?",
        id.to_string().bold().yellow()
    );

    let options = vec![
        InlineOption::new('y', "yes, archive"),
        InlineOption::new('n', "no, skip"),
    ];

    let result = jp_inquire::InlineSelect::new(&prompt, options)
        .with_default('n')
        .prompt(&mut ctx.printer.out_writer());

    match result {
        Ok('y') => Ok(true),
        _ => Ok(false),
    }
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
