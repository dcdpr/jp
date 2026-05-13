use std::fmt::Write as _;

use crossterm::style::Stylize as _;
use inquire::Confirm;
use jp_conversation::ConversationId;
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
        time::TimeThreshold,
    },
    ctx::Ctx,
    format::conversation::DetailsFmt,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Rm {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Remove all conversations created at or after the specified time.
    ///
    /// Accepts a conversation ID (uses its creation timestamp), a relative
    /// duration (e.g. `3w`, `30d`, `6h`), or an absolute date
    /// (e.g. `2026-01-01`). Can be combined with `--until` to remove a range.
    #[arg(long, conflicts_with = "id")]
    from: Option<TimeThreshold>,

    /// Remove all conversations created before the specified time.
    ///
    /// Accepts the same formats as `--from`. The range is half-open
    /// (`--until` is exclusive), so `--from X --until Y` removes everything
    /// in `[X, Y)`.
    #[arg(long, conflicts_with = "id")]
    until: Option<TimeThreshold>,

    /// Do not prompt for confirmation.
    #[arg(long, short = 'y')]
    yes: bool,
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
        if self.from.is_some() || self.until.is_some() {
            handles = ctx
                .workspace
                .conversations()
                .filter(|(id, _)| self.matches(**id))
                .map(|(id, _)| ctx.workspace.acquire_conversation(id))
                .collect::<Result<Vec<_>, _>>()?;

            if handles.is_empty() {
                ctx.printer.println("No conversations match the range.");
                return Ok(());
            }
        }

        for handle in handles {
            remove(ctx, handle, active_id, self.yes).await?;
        }

        ctx.printer.println("Conversation(s) removed.");
        Ok(())
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.from.is_some() || self.until.is_some() {
            ConversationLoadRequest::none()
        } else {
            ConversationLoadRequest::explicit_or_session(&self.target)
        }
    }

    /// Half-open range test on the conversation's creation date. Pure for
    /// testability.
    fn matches(&self, id: ConversationId) -> bool {
        self.from.is_none_or(|t| id.timestamp() >= *t)
            && self.until.is_none_or(|t| id.timestamp() < *t)
    }
}

async fn remove(
    ctx: &mut Ctx,
    handle: ConversationHandle,
    active_id: Option<ConversationId>,
    force: bool,
) -> Output {
    let id = handle.id();
    let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
        LockOutcome::Acquired(lock) => lock,
        LockOutcome::NewConversation => unreachable!("new conversation not allowed"),
        LockOutcome::ForkConversation(_) => unreachable!("fork not allowed"),
    };

    confirm_and_remove(ctx, id, &lock, active_id, force)?;
    ctx.workspace.remove_conversation_with_lock(lock.into_mut());
    Ok(())
}

fn confirm_and_remove(
    ctx: &mut Ctx,
    id: ConversationId,
    lock: &jp_workspace::ConversationLock,
    active_id: Option<ConversationId>,
    force: bool,
) -> Output {
    let conversation = lock.metadata();
    let events = lock.events();
    let mut details = DetailsFmt::new(id)
        .with_last_message_at(events.last().map(|v| v.event.timestamp))
        .with_event_count(events.len())
        .with_title(conversation.title.as_ref())
        .with_last_activated_at(Some(conversation.last_activated_at))
        .with_local_flag(conversation.user)
        .with_active_conversation(active_id.unwrap_or(id))
        .with_pretty_printing(ctx.printer.pretty_printing_enabled());

    if !force {
        details.title = Some(format!(
            "Removing conversation {}",
            id.to_string().bold().yellow()
        ));

        writeln!(ctx.printer.out_writer(), "{details}\n")?;

        let confirm = Confirm::new("Are you sure?")
            .with_default(false)
            .with_confirm_on_input(true)
            .with_help_message("this action cannot be undone");

        match confirm.prompt_with_writer(&mut ctx.printer.out_writer()) {
            Ok(true) => {}
            Ok(false) | Err(_) => return Err(1.into()),
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "rm_tests.rs"]
mod tests;
