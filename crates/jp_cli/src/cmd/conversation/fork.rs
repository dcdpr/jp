use std::sync::Arc;

use jp_conversation::{ConversationStream, Error as ConversationError};
use jp_workspace::{ConversationHandle, ConversationLock};
use tracing::debug;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds, time::TimeThreshold},
    ctx::Ctx,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Fork {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    #[arg(short, long, default_value = "false")]
    activate: bool,

    /// Ignore all conversation events before the specified timestamp.
    ///
    /// Inclusive: an event at exactly this timestamp is kept.
    /// Timestamp can be relative (5days, 2mins, etc) or absolute.
    /// Composes with `--until` to form a half-open `[from, until)` range.
    #[arg(long)]
    from: Option<TimeThreshold>,

    /// Ignore all conversation events at or after the specified timestamp.
    ///
    /// Exclusive: an event at exactly this timestamp is dropped.
    /// Timestamp can be relative (5days, 2mins, etc) or absolute.
    /// Composes with `--from` to form a half-open `[from, until)` range.
    #[arg(long)]
    until: Option<TimeThreshold>,

    /// Fork the first N turns of the conversation.
    /// Defaults to 1.
    ///
    /// Can be combined with `--last` to keep both the leading and trailing
    /// windows while dropping the turns in between.
    #[arg(long, short = 'f')]
    first: Option<Option<usize>>,

    /// Fork the last N turns of the conversation.
    /// Defaults to 1.
    ///
    /// Can be combined with `--first` to keep both the leading and trailing
    /// windows while dropping the turns in between.
    #[arg(long, short = 'l')]
    last: Option<Option<usize>>,

    /// Fork without inheriting any turns.
    ///
    /// The fork keeps the source conversation's full effective configuration
    /// (base config plus every config delta) but starts with zero turns —
    /// equivalent to a fresh conversation whose config matches the source's
    /// current config.
    /// Cannot be combined with the turn-selection or `--compact` flags.
    #[arg(
        short = 'N',
        long,
        conflicts_with_all = ["from", "until", "first", "last", "compact"]
    )]
    no_turns: bool,

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
            // `--no-turns` folds the source's effective config (base + every
            // delta) into a fresh base config; resolving it here lets the
            // fallible `config()` propagate, keeping the closure infallible.
            let collapsed = if self.no_turns {
                Some(
                    ctx.workspace
                        .events(source)?
                        .config()
                        .map_err(ConversationError::from)?,
                )
            } else {
                None
            };

            let lock = fork_conversation(ctx, source, |events| {
                if let Some(config) = &collapsed {
                    // Discard every turn; the merged config becomes the new
                    // base, making this fork identical to a conversation
                    // started fresh with the source's current config.
                    *events = ConversationStream::new(Arc::new(config.clone()))
                        .with_created_at(events.created_at);
                    return;
                }
                // `retain` invalidates compaction overlays from the earliest
                // removed turn onward (overlays confined to the untouched prefix
                // survive), so a time filter that strips whole turns *or* events
                // inside a surviving turn can't leave a stale overlay pointing at
                // — or summarizing — content no longer in the fork. The
                // `--first`/`--last` helpers below inherit the same guarantee.
                events.retain(|event| {
                    self.from.is_none_or(|t| event.timestamp >= *t)
                        && self.until.is_none_or(|t| event.timestamp < *t)
                });

                let first = self.first.map(|v| v.unwrap_or(1));
                let last = self.last.map(|v| v.unwrap_or(1));
                match (first, last) {
                    (None, None) => {}
                    (Some(f), None) => events.retain_first_turns(f),
                    (None, Some(l)) => events.retain_last_turns(l),
                    (Some(f), Some(l)) => events.retain_first_and_last_turns(f, l),
                }
            })?;

            if self.compact.should_compact() {
                let cfg = ctx.config();
                let events_snapshot = lock.events().clone();
                let rules = self
                    .compact
                    .effective_rules(&cfg.conversation.compaction.rules)
                    .map_err(|e| crate::error::Error::Compaction(e.to_string()))?;
                let compactions = super::compact::build_compaction_events(
                    &events_snapshot,
                    &cfg,
                    &rules,
                    super::compact::Bound::Default,
                    super::compact::Bound::Default,
                    // Compaction during a fork is an implicit adjunct; only an
                    // explicit `jp c compact` reports compaction details.
                    None,
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
                        .activate_session_conversation(&lock, session, ctx.now())
            {
                tracing::warn!(%error, "Failed to record activation.");
            }
        }
        ctx.printer.println("Conversation forked.");
        Ok(())
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
