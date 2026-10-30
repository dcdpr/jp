use std::{
    collections::{HashMap, HashSet},
    error::Error,
    sync::Arc,
};

use async_trait::async_trait;
use jp_config::{AppConfig, model::ModelConfig};
use jp_conversation::{ConversationId, ConversationStream, EventId};
use jp_llm::{
    Provider,
    event::NoticeSink,
    provider,
    turn_summary::{self, GeneratedTurnSummary, TurnFingerprint, TurnSummaryRequest},
};
use jp_workspace::{LockResult, Workspace};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use crate::Task;

/// Summarizes the turns of a conversation that have no summary yet.
///
/// [`run`] asks the summary model about the turns as they were when the task
/// was created.
/// [`sync`] stores each summary on its turn's `TurnStart`, unless the turn
/// changed in the meantime or was given a summary some other way.
/// A turn left without one is offered again by the next task.
///
/// [`run`]: Task::run
/// [`sync`]: Task::sync
pub struct TurnSummaryTask {
    /// The conversation the turns belong to.
    conversation_id: ConversationId,

    /// The conversation as it was when the task was created.
    events: ConversationStream,

    /// The turns to summarize, each named by the ID of its `TurnStart`.
    turn_ids: Vec<EventId>,

    /// The model the summaries are generated with.
    model: ModelConfig,

    /// The provider serving `model`.
    provider: Arc<dyn Provider>,

    /// The most turns one task summarizes.
    batch_size: usize,

    /// Output ceiling for the summary request, from
    /// `assistant.request.max_response_bytes`.
    /// `None` leaves the response unbounded.
    max_response_bytes: Option<u64>,

    /// Where the summary request's provider notices go, such as a switch onto
    /// another credential.
    notices: NoticeSink,

    /// The summaries the model returned, filled in by `run`.
    summaries: Vec<GeneratedTurnSummary>,
}

impl TurnSummaryTask {
    /// Prepare a task for the turns in `events` that have no summary.
    ///
    /// Returns `None` when every turn already has one, or when
    /// `conversation.turn_summary.generate.batch_size` is `0`, without setting
    /// up the summary model's provider.
    ///
    /// # Errors
    ///
    /// Returns an error when the summary model's provider cannot be set up,
    /// such as when its API key environment variable is unset.
    pub fn new(
        conversation_id: ConversationId,
        events: &ConversationStream,
        config: &AppConfig,
        notices: NoticeSink,
    ) -> Result<Option<Self>, Box<dyn Error + Send + Sync>> {
        let batch_size = config.conversation.turn_summary.generate.batch_size;
        let turn_ids = unsummarized_turns(events);
        if batch_size == 0 || turn_ids.is_empty() {
            return Ok(None);
        }

        let model = turn_summary::resolve_model(config);

        // Set up here rather than in `run`, so a misconfigured provider fails
        // before the query commits to waiting for the task at teardown.
        let provider = provider::get_provider(model.id.resolved().provider, &config.providers.llm)?;

        Ok(Some(Self {
            conversation_id,
            events: events.clone(),
            turn_ids,
            model,
            provider: Arc::from(provider),
            batch_size,
            max_response_bytes: config.assistant.request.max_response_bytes.bytes(),
            notices,
            summaries: vec![],
        }))
    }

    /// Ask the summary model about the task's turns, keeping what it returns.
    async fn summarize(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(
            conversation_id = %self.conversation_id,
            candidates = self.turn_ids.len(),
            "Summarizing turns."
        );

        let details = self
            .provider
            .model_details(&self.model.id.resolved().name)
            .await?;

        self.summaries =
            turn_summary::generate(self.provider.as_ref(), &details, TurnSummaryRequest {
                events: self.events.clone(),
                turn_ids: self.turn_ids.clone(),
                model: self.model.clone(),
                batch_size: self.batch_size,
                max_response_bytes: self.max_response_bytes,
                notices: self.notices.clone(),
            })
            .await?;

        trace!(summaries = self.summaries.len(), "Received turn summaries.");
        Ok(())
    }
}

#[async_trait]
impl Task for TurnSummaryTask {
    fn name(&self) -> &'static str {
        "turn_summary"
    }

    async fn run(
        mut self: Box<Self>,
        token: CancellationToken,
    ) -> Result<Box<dyn Task>, Box<dyn Error + Send + Sync>> {
        let id = self.conversation_id;
        jp_macro::select!(
            token.cancelled(),
            |_cancel| {
                trace!(conversation_id = %id, "Turn summary task cancelled.");
            },
            self.summarize(),
            |result| {
                if let Err(error) = result {
                    warn!(?error, conversation_id = %id, "Turn summary task failed.");
                    return Err(error);
                }
            }
        );

        Ok(self)
    }

    async fn sync(
        self: Box<Self>,
        ctx: &mut Workspace,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let Self {
            conversation_id,
            summaries,
            ..
        } = *self;

        if summaries.is_empty() {
            return Ok(());
        }

        let Ok(handle) = ctx.acquire_conversation(&conversation_id) else {
            debug!(%conversation_id, "Conversation is gone; dropping its turn summaries.");
            return Ok(());
        };

        // Lock the conversation before writing. The query's lock has been
        // released by the time task sync runs, and taking the lock reloads the
        // conversation, so the check below sees edits made since the task
        // started.
        let lock = match ctx.lock_conversation(handle, None)? {
            LockResult::Acquired(lock) => lock,
            LockResult::AlreadyLocked(_) => {
                warn!(
                    %conversation_id,
                    "Could not lock conversation for turn summaries, skipping."
                );
                return Ok(());
            }
        };

        let summaries = lock.with_events(|events| still_current(events, summaries));
        if summaries.is_empty() {
            return Ok(());
        }

        let stored = lock.into_mut().update_events_and_flush(|events| {
            let mut stored = 0;
            for GeneratedTurnSummary {
                turn_id, summary, ..
            } in summaries
            {
                if events.set_turn_summary(&turn_id, summary) {
                    stored += 1;
                }
            }

            stored
        });

        match stored {
            Ok(count) => debug!(%conversation_id, count, "Stored turn summaries."),
            Err(error) => warn!(%error, %conversation_id, "Failed to persist turn summaries."),
        }

        Ok(())
    }
}

/// The IDs of the `TurnStart` entries in `events` that carry no summary, in
/// stream order.
fn unsummarized_turns(events: &ConversationStream) -> Vec<EventId> {
    events
        .iter_events_by_turn()
        .filter(|entry| {
            entry
                .as_turn_start()
                .is_some_and(|turn_start| turn_start.summary.is_none())
        })
        .map(|entry| entry.event_id.clone())
        .collect()
}

/// Keep the summaries whose turn reads as it did when the model read it, and
/// still has no summary.
///
/// A turn edited or removed since then would be described by a summary of
/// content it no longer holds.
/// A turn given a summary since then keeps that one.
fn still_current(
    events: &ConversationStream,
    summaries: Vec<GeneratedTurnSummary>,
) -> Vec<GeneratedTurnSummary> {
    let turn_ids: Vec<EventId> = summaries.iter().map(|s| s.turn_id.clone()).collect();
    let fingerprints: HashMap<EventId, TurnFingerprint> =
        turn_summary::transcripts(events, &turn_ids)
            .into_iter()
            .map(|transcript| (transcript.turn_id().clone(), transcript.fingerprint()))
            .collect();
    let unsummarized: HashSet<EventId> = unsummarized_turns(events).into_iter().collect();

    summaries
        .into_iter()
        .filter(|summary| {
            if fingerprints.get(&summary.turn_id) != Some(&summary.fingerprint) {
                debug!(
                    turn_id = %summary.turn_id,
                    "Turn changed or was removed since it was read; dropping its summary."
                );
                return false;
            }

            if !unsummarized.contains(&summary.turn_id) {
                debug!(
                    turn_id = %summary.turn_id,
                    "Turn was given a summary since it was read; keeping that one."
                );
                return false;
            }

            true
        })
        .collect()
}

#[cfg(test)]
#[path = "turn_summary_tests.rs"]
mod tests;
