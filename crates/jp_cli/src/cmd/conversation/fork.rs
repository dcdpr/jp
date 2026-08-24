use std::sync::Arc;

use jp_conversation::{ConversationId, ConversationStream, Error as ConversationError};
use jp_inquire::prompt::TerminalPromptBackend;
use jp_printer::Printer;
use jp_storage::backend::Projection;
use jp_workspace::{ConversationHandle, ConversationLock};
use serde_json::Value;
use tracing::debug;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        label::resolve::{Resolver, Trigger},
        turn_selection::TurnSelection,
    },
    ctx::Ctx,
    output::print_json,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Fork {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    #[arg(short, long, default_value = "false")]
    activate: bool,

    /// Which turns the fork inherits.
    ///
    /// Without any selector, the fork inherits every turn.
    /// `--from`/`--to` bound the inherited range; `--first N`/`--last N`
    /// inherit the first or last N turns (both together keep each window and
    /// drop the turns in between); `--turn N` inherits a single turn, or
    /// `--turn A..B` an inclusive range.
    /// `--keep-first`/`--keep-last` drop turns at either end of the selection.
    #[command(flatten)]
    range: TurnSelection,

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
        conflicts_with_all = [
            "from", "to", "turn", "first", "last", "keep_first", "keep_last", "compact",
        ]
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
        let mut forked = Vec::with_capacity(handles.len());

        // A fork is persisted as soon as it is created, so work that fails
        // after that point leaves it behind. Reporting the IDs either way keeps
        // the created conversations addressable; the error still propagates, so
        // the exit status says the run did not finish.
        let result = self.fork_each(ctx, handles, &mut forked).await;
        print_forked(&ctx.printer, &forked);

        result
    }

    /// Fork every source, recording each new conversation's ID as it is
    /// created.
    async fn fork_each(
        &self,
        ctx: &mut Ctx,
        handles: &[ConversationHandle],
        forked: &mut Vec<ConversationId>,
    ) -> Output {
        self.range.validate()?;

        // Reject an out-of-range `--turn` against every source before forking any
        // of them: `fork` accepts multiple sources, and checking inside the loop
        // would leave the earlier forks created when a later source turns out to
        // be too short.
        //
        // A source that fails to *load* is deliberately skipped rather than
        // propagated here. That failure is not predictable from the flags, so it
        // belongs to the mutation loop below, which reports the forks it already
        // created before returning the error.
        for source in handles {
            if let Ok(events) = ctx.workspace.events(source) {
                self.range.check_turn_range(events.turn_count())?;
            }
        }

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
                if !self.range.is_set() {
                    return;
                }

                // `retain_turns` invalidates compaction overlays from the
                // earliest removed turn onward (overlays confined to the
                // untouched prefix survive), so a selection that drops turns
                // can't leave a stale overlay pointing at — or summarizing —
                // content no longer in the fork.
                let selected = self.range.resolve(events);
                events.retain_turns(|index| selected.contains(index));
            })
            .await?;

            if self.compact.should_compact() {
                let cfg = ctx.config();
                let events_snapshot = lock.events().clone();
                let rules = self
                    .compact
                    .effective_rules(&cfg.conversation.compaction.rules)
                    .map_err(|e| crate::error::Error::Compaction(e.to_string()))?;
                // The fork's turn selection has already been applied to the
                // stream, so compaction covers the whole fork.
                let compactions = super::compact::build_compaction_events(
                    &events_snapshot,
                    &cfg,
                    &rules,
                    &TurnSelection::default(),
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

            forked.push(lock.id());
        }

        Ok(())
    }
}

/// Report the conversations the fork created.
///
/// Text output is one ID per line, in source order, so `FORK_ID="$(jp c fork
/// ...)"` captures the ID with nothing to strip.
/// JSON output is a top-level array of IDs.
fn print_forked(printer: &Printer, ids: &[ConversationId]) {
    if printer.format().is_json() {
        let ids = ids.iter().map(|id| Value::String(id.to_string())).collect();
        print_json(printer, &Value::Array(ids));
        return;
    }

    for id in ids {
        printer.println(id.to_string());
    }
}

/// Fork a conversation and return the new conversation's lock.
///
/// The fork inherits the source's labels, then every `apply_on.fork` rule is
/// re-resolved over them.
/// Resolution happens *before* the fork is created, so a rule that needs
/// confirmation and finds no terminal to ask on fails without leaving an
/// unreported conversation behind.
pub(crate) async fn fork_conversation(
    ctx: &mut Ctx,
    source: &ConversationHandle,
    mut filter: impl FnMut(&mut ConversationStream),
) -> crate::Result<ConversationLock> {
    let now = ctx.now();

    // Resolved up front: everything below this line writes to disk.
    let config = ctx.config();
    let prompts = TerminalPromptBackend;
    let resolved = Resolver::new(
        &config.conversation.labels,
        ctx.workspace.root(),
        ctx.term.is_tty,
        &ctx.printer,
        &prompts,
    )
    .automatic(Trigger::Fork)
    .await?;

    let mut new_conversation = ctx.workspace.metadata(source)?.clone();
    new_conversation.last_activated_at = now;
    new_conversation.expires_at = None;
    // A rule replaces the key's set; keys with no matching rule are inherited
    // untouched.
    for (key, values) in resolved {
        new_conversation.labels.set(key, values);
    }

    let mut new_events = ctx.workspace.events(source)?.clone().with_created_at(now);

    filter(&mut new_events);
    new_events.sanitize();

    // Inherit the source's storage locality so forking a `--local` conversation
    // doesn't project the fork into the workspace.
    let projection = ctx
        .workspace
        .conversation_presence(&source.id())
        .map_or(Projection::Projected, Projection::from);

    let lock = ctx.workspace.create_and_lock_conversation_with_projection(
        new_conversation,
        new_events.base_config(),
        ctx.session.as_ref(),
        projection,
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
