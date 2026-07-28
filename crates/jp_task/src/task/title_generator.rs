use std::error::Error;

use async_trait::async_trait;
use jp_config::{AppConfig, model::ModelConfig, providers::llm::LlmProviderConfig};
use jp_conversation::{ConversationId, ConversationStream};
use jp_llm::{
    provider,
    title::{self, TitleRequest},
};
use jp_workspace::Workspace;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

use crate::Task;

#[derive(Debug)]
pub struct TitleGeneratorTask {
    pub conversation_id: ConversationId,
    pub model: ModelConfig,
    pub providers: LlmProviderConfig,
    pub events: ConversationStream,
    pub title: Option<String>,
    /// Whether the invoking process is attached to a terminal.
    /// When `false`, the OSC-2 title-update side effect on task sync is
    /// suppressed — the bytes would otherwise leak into a captured pipe.
    pub is_tty: bool,
}

impl TitleGeneratorTask {
    pub fn new(
        conversation_id: ConversationId,
        events: ConversationStream,
        config: &AppConfig,
        is_tty: bool,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let model = title::resolve_model(config, None);

        // Fail fast on a misconfigured title provider (e.g. a missing API
        // key environment variable). Without this, the failure only surfaces
        // inside the spawned task, after the query has already committed to
        // waiting for it at teardown.
        provider::preflight(model.id.resolved().provider, &config.providers.llm)?;

        Ok(Self {
            conversation_id,
            model,
            providers: config.providers.llm.clone(),
            events,
            title: None,
            is_tty,
        })
    }

    async fn update_title(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(conversation_id = %self.conversation_id, "Updating conversation title.");

        let model_id = self.model.id.resolved().clone();
        let provider = provider::get_provider(model_id.provider, &self.providers)?;
        let details = provider.model_details(&model_id.name).await?;

        let titles = title::generate(provider.as_ref(), &details, TitleRequest {
            events: self.events.clone(),
            model: self.model.clone(),
            count: 1,
            rejected: vec![],
        })
        .await?;

        trace!(?titles, "Received conversation titles.");
        self.title = titles.into_iter().next();
        if self.title.is_none() {
            warn!(
                conversation_id = %self.conversation_id,
                "No title in the generation response."
            );
        }

        Ok(())
    }
}

#[async_trait]
impl Task for TitleGeneratorTask {
    fn name(&self) -> &'static str {
        "title_generator"
    }

    async fn run(
        mut self: Box<Self>,
        token: CancellationToken,
    ) -> Result<Box<dyn Task>, Box<dyn Error + Send + Sync>> {
        let id = self.conversation_id;
        jp_macro::select!(
            token.cancelled(),
            |_cancel| {
                trace!(conversation_id = %id, "Title generator task cancelled.");
            },
            self.update_title(),
            |result| {
                match result {
                    Ok(()) => trace!(conversation_id = %id, "Title generator task completed."),
                    Err(error) => {
                        warn!(?error, conversation_id = %id, "Title generator task failed.");
                        return Err(error);
                    }
                }
            }
        );

        Ok(self)
    }

    async fn sync(
        self: Box<Self>,
        ctx: &mut Workspace,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if let Ok(handle) = ctx.acquire_conversation(&self.conversation_id) {
            // Lock the conversation before writing the title. The query's
            // lock has been released by the time task sync runs.
            let lock = match ctx.lock_conversation(handle, None)? {
                jp_workspace::LockResult::Acquired(lock) => lock,
                jp_workspace::LockResult::AlreadyLocked(_) => {
                    warn!(
                        conversation_id = %self.conversation_id,
                        "Could not lock conversation for title update, skipping."
                    );
                    return Ok(());
                }
            };
            let mut conv = lock.into_mut();
            conv.update_metadata(|m| m.title = self.title.clone());
            if let Err(e) = conv.flush() {
                warn!(error = %e, "Failed to persist title update.");
            }
        }

        // Update terminal title now that we have a generated name. Only
        // emit the OSC-2 sequence when the original invocation was on a
        // terminal — writing it into a captured pipe pollutes the output
        // without any visible effect.
        if self.is_tty
            && let Some(title) = &self.title
        {
            let display = format!("{}: {title}", self.conversation_id);
            jp_term::osc::set_title(display);
        }

        Ok(())
    }
}
