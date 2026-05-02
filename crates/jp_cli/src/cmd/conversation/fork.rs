use jp_conversation::ConversationStream;
use jp_workspace::{ConversationHandle, ConversationLock};
use tracing::debug;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds, time::TimeThreshold},
    ctx::{Ctx, IntoPartialAppConfig},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Fork {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    #[arg(short, long, default_value = "false")]
    activate: bool,

    /// Ignore all conversation events *before* the specified timestamp.
    ///
    /// Timestamp can be relative (5days, 2mins, etc) or absolute. Can be used
    /// in combination with `--until`.
    #[arg(long)]
    from: Option<TimeThreshold>,

    /// Ignore all conversation events *after* the specified timestamp.
    ///
    /// Timestamp can be relative (5days, 2mins, etc) or absolute. Can be used
    /// in combination with `--until`.
    #[arg(long)]
    until: Option<TimeThreshold>,

    /// Fork the last N turns of the conversation. Defaults to 1.
    #[arg(long, short = 'l')]
    last: Option<Option<usize>>,

    /// Compact the forked conversation.
    #[command(flatten)]
    compact: crate::cmd::compact_flag::CompactFlag,

    /// Set a custom title for the forked conversation.
    #[arg(long, short)]
    title: Option<String>,
}

impl Fork {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: &[ConversationHandle]) -> Output {
        for source in handles {
            let lock = fork_conversation(ctx, source, |events| {
                events.retain(|event| {
                    self.from.is_none_or(|t| event.timestamp >= *t)
                        && self.until.is_none_or(|t| event.timestamp <= *t)
                });

                if let Some(last) = self.last {
                    events.retain_last_turns(last.unwrap_or(1));
                }
            })?;

            if self.compact.should_compact() {
                let cfg = ctx.config();
                let events_snapshot = lock.events().clone();
                let compactions = super::compact::build_compaction_events_from_config(
                    &events_snapshot,
                    &cfg,
                    None,
                    None,
                    &ctx.printer,
                )
                .await?;
                for compaction in compactions {
                    lock.as_mut()
                        .update_events(|events| events.add_compaction(compaction));
                }
            }

            if let Some(title) = &self.title {
                lock.as_mut().update_metadata(|m| {
                    m.title = Some(title.clone());
                });
            }

            if self.activate
                && let Some(session) = &ctx.session
                && let Err(error) =
                    ctx.workspace
                        .activate_session_conversation(session, lock.id(), ctx.now())
            {
                tracing::warn!(%error, "Failed to write session mapping.");
            }
        }
        ctx.printer.println("Conversation forked.");
        Ok(())
    }
}

impl IntoPartialAppConfig for Fork {
    fn apply_cli_config(
        &self,
        _workspace: Option<&jp_workspace::Workspace>,
        mut partial: jp_config::PartialAppConfig,
        _merged_config: Option<&jp_config::PartialAppConfig>,
        _handles: &[jp_workspace::ConversationHandle],
    ) -> Result<jp_config::PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        self.compact.apply_to_config(&mut partial);
        Ok(partial)
    }
}

/// Fork a conversation and return the new conversation's lock.
pub(crate) fn fork_conversation(
    ctx: &mut Ctx,
    source: &ConversationHandle,
    mut filter: impl FnMut(&mut ConversationStream),
) -> crate::Result<ConversationLock> {
    let now = ctx.now();

    let mut new_conversation = ctx.workspace.metadata(source)?.clone();
    new_conversation.last_activated_at = now;
    new_conversation.expires_at = None;

    let mut new_events = ctx.workspace.events(source)?.clone().with_created_at(now);

    filter(&mut new_events);
    new_events.sanitize();

    let lock = ctx.workspace.create_and_lock_conversation(
        new_conversation,
        new_events.base_config(),
        ctx.session.as_ref(),
    )?;

    lock.as_mut()
        .update_events(|events| events.extend(new_events));

    debug!(
        source = source.id().to_string(),
        fork = lock.id().to_string(),
        "Forked conversation."
    );

    Ok(lock)
}

#[cfg(test)]
#[path = "fork_tests.rs"]
mod tests;
