use crossterm::style::Stylize as _;
use jp_conversation::{Conversation, ConversationId};
use jp_inquire::InlineOption;
use jp_workspace::{ConversationHandle, Workspace};

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
        time::{CreationRange, TimeThreshold},
    },
    ctx::Ctx,
    shared::confirm::ConfirmFlag,
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
///
/// Use `--from`/`--until` to archive a range of conversations by creation date,
/// or `--inactive-since` to archive everything unused since a given time.
/// The three filters AND together when combined.
///
/// Archived conversations are hidden from listings and pickers.
/// Use `jp c ls --archived` to list them, `jp c unarchive` to restore them, or
/// `jp c use archived` to unarchive and activate.
#[derive(Debug, clap::Args)]
pub(crate) struct Archive {
    #[command(flatten)]
    target: PositionalIds<false, true>,

    /// Archive all conversations created in a `[--from, --until)` range.
    #[command(flatten)]
    range: CreationRange,

    /// Archive all conversations inactive since a given time.
    ///
    /// Accepts the same formats as `--from`.
    /// Filters on `last_activated_at` (when the conversation was last used)
    /// rather than its creation date, which makes this distinct from `--until`.
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

        let preference = self.confirm.preference();
        let multi = handles.len() > 1;
        for handle in handles {
            let id = handle.id();

            if !confirm_archive(ctx, &id, preference, multi)? {
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

/// Decide whether to archive `id`, prompting when appropriate.
///
/// Returns `true` to proceed, `false` to skip.
/// `preference` is the resolved `--confirm` / `--no-confirm` choice:
/// `Some(true)` always prompts, `Some(false)` never prompts, and `None` prompts
/// only for pinned or active conversations, or when archiving more than one
/// conversation at once (`multi`).
/// The conversation title, when known, is shown so a bulk selection can be
/// verified.
fn confirm_archive(
    ctx: &mut Ctx,
    id: &ConversationId,
    preference: Option<bool>,
    multi: bool,
) -> Result<bool, crate::error::Error> {
    if preference == Some(false) {
        return Ok(true);
    }

    let handle = ctx.workspace.acquire_conversation(id)?;
    let meta = ctx.workspace.metadata(&handle)?;

    let is_active = ctx
        .session
        .as_ref()
        .and_then(|s| ctx.workspace.session_active_conversation(s))
        == Some(*id);
    let is_pinned = meta.is_pinned();

    // Default (`None`) prompts only for pinned, active, or bulk archives;
    // `--confirm` (`Some(true)`) prompts for everything.
    if preference != Some(true) && !is_active && !is_pinned && !multi {
        return Ok(true);
    }

    // Active subsumes pinned in the prompt; with `--confirm` a plain
    // conversation gets an unqualified prompt. The title, when known, is shown
    // so a bulk selection can be verified.
    let id_label = id.to_string().bold().yellow();
    let title = meta
        .title
        .as_deref()
        .map(|t| format!(" \"{t}\""))
        .unwrap_or_default();
    let prompt = if is_active {
        format!("Archive the active conversation {id_label}{title}?")
    } else if is_pinned {
        format!("Archive the pinned conversation {id_label}{title}?")
    } else {
        format!("Archive conversation {id_label}{title}?")
    };

    let options = vec![
        InlineOption::new('y', "yes, archive"),
        InlineOption::new('n', "no, skip"),
    ];

    let result = jp_inquire::InlineSelect::new(&prompt, options)
        .with_default('n')
        .prompt(&mut ctx.printer.prompt_writer());

    match result {
        Ok('y') => Ok(true),
        _ => Ok(false),
    }
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
