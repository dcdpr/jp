use chrono::{DateTime, Utc};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_inquire::InlineOption;
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::{ConversationIds as _, PositionalIds},
        lock::{LockOutcome, LockRequest, acquire_lock},
        target::{ConversationTarget, PickerFilter},
        time::TimeThreshold,
    },
    ctx::Ctx,
};

/// Archive conversations.
///
/// Without IDs, shows a picker of conversations. With IDs, archives each one.
/// Prompts for confirmation when archiving pinned or active conversations.
///
/// Use `--inactive-since` to archive all conversations that haven't been
/// used since a given time or duration (e.g. `3w`, `2026-01-01`).
///
/// Archived conversations are hidden from listings and pickers. Use
/// `jp c ls --archived` to list them, `jp c unarchive` to restore them,
/// or `jp c use archived` to unarchive and activate.
#[derive(Debug, clap::Args)]
pub(crate) struct Archive {
    #[command(flatten)]
    target: PositionalIds<false, true>,

    /// Archive all conversations inactive since a given time.
    ///
    /// Accepts a relative duration (e.g. `3w`, `30d`, `6h`) or an absolute
    /// date (e.g. `2026-01-01`). Archives every conversation whose last
    /// activity is before the computed threshold.
    #[arg(long, conflicts_with = "id")]
    inactive_since: Option<TimeThreshold>,
}

impl Archive {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.inactive_since.is_some() {
            // --inactive-since resolves conversations internally.
            ConversationLoadRequest::none()
        } else {
            let targets = self.target.ids();
            if targets.is_empty() {
                ConversationLoadRequest::explicit(vec![ConversationTarget::Picker(
                    PickerFilter::default(),
                )])
            } else {
                ConversationLoadRequest::explicit(targets.to_vec())
            }
        }
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        if let Some(threshold) = self.inactive_since {
            return self.run_inactive_since(ctx, *threshold).await;
        }

        for handle in handles {
            let id = handle.id();

            if !confirm_archive(ctx, &id)? {
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

    /// Archive all conversations with `last_activated_at` before `cutoff`.
    async fn run_inactive_since(&self, ctx: &mut Ctx, cutoff: DateTime<Utc>) -> Output {
        let ids: Vec<_> = ctx
            .workspace
            .conversations()
            .filter(|(_, c)| c.last_activated_at < cutoff)
            .map(|(id, _)| *id)
            .collect();

        if ids.is_empty() {
            ctx.printer.println("No conversations match the threshold.");
            return Ok(());
        }

        for id in ids {
            let Ok(handle) = ctx.workspace.acquire_conversation(&id) else {
                continue;
            };

            if !confirm_archive(ctx, &id)? {
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
