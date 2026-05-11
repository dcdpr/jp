use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_workspace::{ConversationHandle, LockResult};
use tracing::{debug, warn};

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::{ConversationIds as _, PositionalIds},
        target::ConversationTarget,
    },
    ctx::Ctx,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Use {
    #[command(flatten)]
    target: PositionalIds<true, false>,
}

impl Use {
    /// Whether the targets resolve against the archive partition.
    fn is_archived(&self) -> bool {
        self.target
            .ids()
            .iter()
            .any(ConversationTarget::is_archived)
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        // Archive targets bypass the normal resolution pipeline — the ID isn't
        // in the workspace index yet. We resolve + unarchive + activate in one
        // step.
        if self.is_archived() {
            return self.run_unarchive(ctx);
        }

        let handle = handles.into_iter().next().expect("Use requires a handle");
        Self::run_activate_inner(ctx, handle)
    }

    fn run_activate_inner(ctx: &mut Ctx, handle: ConversationHandle) -> Output {
        let id = handle.id();

        let active_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));

        if active_id == Some(id) {
            let id_fmt = id.to_string().bold().yellow();
            ctx.printer
                .println(format!("Already active conversation: {id_fmt}"));
            return Ok(());
        }

        let Some(session) = &ctx.session else {
            return Err((
                1,
                "No session identity available. Set $JP_SESSION or run in a terminal with \
                 automatic session detection."
                    .to_string(),
            )
                .into());
        };

        // Try to acquire the conversation lock non-blocking. On contention
        // (another process is mid-query on this conversation), skip the
        // metadata bump — we can't write `last_activated_at` while someone else
        // holds the lock, and we don't want to block behind a streaming query
        // just to record the activation time in the metadata.
        //
        // The on-disk `last_activated_at` reflects the lock holder's activation
        // time (typically recent), which is close enough for sorting and
        // archive cutoffs in the common case. We still record this session's
        // mapping so the user's intent ("X is now my active conversation") is
        // captured immediately, and the `SessionHistoryEntry::activated_at` we
        // write here carries the exact `now`.
        let now = ctx.now();
        let result = match ctx.workspace.lock_conversation(handle, Some(session))? {
            LockResult::Acquired(lock) => ctx
                .workspace
                .activate_session_conversation(&lock, session, now),
            LockResult::AlreadyLocked(_) => {
                debug!(
                    %id,
                    "Conversation locked by another process; recording session activation only."
                );
                ctx.workspace.record_session_activation(session, id, now)
            }
        };
        if let Err(error) = result {
            warn!(%error, "Failed to record activation.");
        }

        let from = active_id.map_or_else(
            || "(none)".grey().to_string(),
            |id| id.to_string().bold().grey().to_string(),
        );
        let to = id.to_string().bold().yellow();
        let title_suffix = conversation_title(ctx, id)
            .map(|t| format!(": {}", t.yellow()))
            .unwrap_or_default();

        ctx.printer.println(format!(
            "Switched active conversation from {from} to {to}{title_suffix}"
        ));

        Ok(())
    }

    /// Resolve an archived conversation target, unarchive it, and activate it.
    fn run_unarchive(&self, ctx: &mut Ctx) -> Output {
        // Resolve the archive target to a concrete ID.
        let id = self
            .target
            .ids()
            .iter()
            .find_map(|t| {
                t.resolve(&ctx.workspace, ctx.session.as_ref())
                    .ok()
                    .and_then(|ids| ids.into_iter().next())
            })
            .ok_or_else(|| {
                crate::error::Error::NotFound("conversation", "no archived conversations".into())
            })?;

        let handle = ctx.workspace.unarchive_conversation(&id)?;

        let id_fmt = id.to_string().bold().yellow();
        ctx.printer
            .println(format!("Unarchived conversation {id_fmt}"));

        Self::run_activate_inner(ctx, handle)
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.is_archived() {
            // Archive targets are handled internally — skip normal resolution.
            ConversationLoadRequest::none()
        } else {
            ConversationLoadRequest::explicit_or_previous(&self.target)
        }
    }
}

fn conversation_title(ctx: &Ctx, id: ConversationId) -> Option<String> {
    let h = ctx.workspace.acquire_conversation(&id).ok()?;
    ctx.workspace.metadata(&h).ok()?.title.clone()
}

#[cfg(test)]
#[path = "use_tests.rs"]
mod tests;
