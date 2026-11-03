use std::{
    collections::{HashMap, HashSet},
    error::Error,
    sync::Arc,
};

use async_trait::async_trait;
use jp_config::AppConfig;
use jp_conversation::{ConversationId, ConversationStream, EventId};
use jp_llm::{
    Provider,
    event::NoticeSink,
    provider,
    turn_summary::{self, GeneratedTurnSummary, TurnFingerprint, TurnSummaryRequest},
};
use jp_workspace::{ConversationLock, LockResult, Workspace};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use crate::Task;

/// Background task that generates summaries for completed turns lacking one.
///
/// [`Task::run`] queries the configured summary model against a snapshot of the
/// conversation stream taken before the active turn starts. [`Task::sync`]
/// persists each returned summary on its `TurnStart` entry if the turn still
/// has no summary and its transcript fingerprint is unchanged.
pub struct TurnSummaryTask {
    conversation_id: ConversationId,
    provider: Arc<dyn Provider>,
    request: Option<TurnSummaryRequest>,
    summaries: Vec<GeneratedTurnSummary>,
}

impl TurnSummaryTask {
    /// Construct a summary task for unsummarized turns in `events`.
    ///
    /// Returns `Ok(None)` without initializing a provider if `batch_size` is `0`
    /// or every turn in `events` already has a summary.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary model's provider cannot be initialized
    /// (for example, when its API key environment variable is unset).
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
        let provider = provider::get_provider(model.id.resolved().provider, &config.providers.llm)?;

        Ok(Some(Self {
            conversation_id,
            provider: Arc::from(provider),
            request: Some(TurnSummaryRequest {
                events: events.clone(),
                turn_ids,
                model,
                batch_size,
                max_response_bytes: config.assistant.request.max_response_bytes.bytes(),
                notices,
            }),
            summaries: vec![],
        }))
    }

    async fn summarize(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let Some(request) = self.request.take() else {
            return Ok(());
        };

        trace!(
            conversation_id = %self.conversation_id,
            candidates = request.turn_ids.len(),
            "Summarizing turns."
        );

        let details = self
            .provider
            .model_details(&request.model.id.resolved().name)
            .await?;

        self.summaries = turn_summary::generate(self.provider.as_ref(), &details, request).await?;

        trace!(summaries = self.summaries.len(), "Received turn summaries.");
        Ok(())
    }

    /// Run the summary request directly and return the generated summaries.
    ///
    /// Used by callers that manage conversation locking outside [`TaskHandler`].
    ///
    /// # Errors
    ///
    /// Returns an error if the summary model request fails.
    ///
    /// [`TaskHandler`]: crate::TaskHandler
    pub async fn generate(mut self) -> Result<TurnSummaries, Box<dyn Error + Send + Sync>> {
        self.summarize().await?;

        Ok(TurnSummaries {
            conversation_id: self.conversation_id,
            summaries: self.summaries,
        })
    }
}

/// Generated turn summaries awaiting persistence.
///
/// When stored, each summary is written only if its turn still has no summary
/// and its transcript fingerprint has not changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummaries {
    pub conversation_id: ConversationId,
    pub summaries: Vec<GeneratedTurnSummary>,
}

impl TurnSummaries {
    /// Persist the summaries through an existing lock on their conversation.
    pub fn store(self, lock: &ConversationLock) {
        let Self {
            conversation_id,
            summaries,
        } = self;

        if lock.id() != conversation_id {
            warn!(
                %conversation_id,
                locked = %lock.id(),
                "Turn summaries offered to another conversation's lock; dropping them."
            );
            return;
        }

        let summaries = lock.with_events(|events| still_current(events, summaries));
        if summaries.is_empty() {
            return;
        }

        let stored = lock.as_mut().update_events_and_flush(|events| {
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
    }

    /// Acquire a non-blocking lock on the conversation in `workspace` and
    /// persist the summaries.
    ///
    /// If the conversation was removed or is locked by another process, the
    /// summaries are skipped and will be regenerated on a subsequent query.
    ///
    /// # Errors
    ///
    /// Returns an error if locking the conversation fails.
    pub fn store_in(self, workspace: &Workspace) -> Result<(), Box<dyn Error + Send + Sync>> {
        let conversation_id = self.conversation_id;
        if self.summaries.is_empty() {
            return Ok(());
        }

        let Ok(handle) = workspace.acquire_conversation(&conversation_id) else {
            debug!(%conversation_id, "Conversation removed; dropping turn summaries.");
            return Ok(());
        };

        match workspace.lock_conversation(handle, None)? {
            LockResult::Acquired(lock) => self.store(&lock),
            LockResult::AlreadyLocked(_) => warn!(
                %conversation_id,
                "Could not lock conversation for turn summaries, skipping."
            ),
        }

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

        TurnSummaries {
            conversation_id,
            summaries,
        }
        .store_in(ctx)
    }
}

/// Return the `EventId`s of `TurnStart` entries in `events` that have no
/// summary, in stream order.
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

/// Retain only summaries whose target turn still has no summary and still
/// matches its [`TurnFingerprint`].
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
                    "Turn changed or was removed; dropping generated summary."
                );
                return false;
            }

            if !unsummarized.contains(&summary.turn_id) {
                debug!(
                    turn_id = %summary.turn_id,
                    "Turn already has a summary; keeping existing summary."
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
