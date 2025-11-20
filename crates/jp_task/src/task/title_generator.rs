use std::error::Error;

use async_trait::async_trait;
use jp_config::{
    AppConfig,
    model::{
        ModelConfig,
        id::ModelIdConfig,
        parameters::{CustomReasoningConfig, ParametersConfig, ReasoningEffort},
    },
    providers::llm::LlmProviderConfig,
};
use jp_conversation::{ConversationId, ConversationStream};
use jp_llm::{provider, structured};
use jp_workspace::Workspace;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

use crate::Task;

#[derive(Debug)]
pub struct TitleGeneratorTask {
    pub conversation_id: ConversationId,
    pub model_id: ModelIdConfig,
    pub parameters: ParametersConfig,
    pub providers: LlmProviderConfig,
    pub events: ConversationStream,
    pub title: Option<String>,
}

impl TitleGeneratorTask {
    pub fn new(
        conversation_id: ConversationId,
        events: ConversationStream,
        config: &AppConfig,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        // Prefer the title generation model id, otherwise use the assistant
        // model id.
        let mut model = config
            .conversation
            .title
            .generate
            .model
            .clone()
            .unwrap_or_else(|| ModelConfig {
                id: config.assistant.model.id.clone(),
                parameters: ParametersConfig::default(),
            });

        // Get the model ID from the model configuration.
        let model_id = model.id.finalize(&config.providers.llm.aliases)?;

        // If reasoning is explicitly enabled for title generation, use it,
        // otherwise limit it to
        if model.parameters.reasoning.is_none() {
            model.parameters.reasoning = Some(
                CustomReasoningConfig {
                    effort: ReasoningEffort::Low,
                    exclude: true,
                }
                .into(),
            );
        }

        Ok(Self {
            conversation_id,
            model_id,
            parameters: model.parameters,
            providers: config.providers.llm.clone(),
            events,
            title: None,
        })
    }

    async fn update_title(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(conversation_id = %self.conversation_id, "Updating conversation title.");

        let provider = provider::get_provider(self.model_id.provider, &self.providers)?;
        let query = structured::titles::titles(1, self.events.clone(), &[])?;
        let titles: Vec<_> =
            structured::completion(provider.as_ref(), &self.model_id, &self.parameters, query)
                .await?;

        trace!(titles = ?titles, "Received conversation titles.");
        if let Some(title) = titles.into_iter().next() {
            self.title = Some(title);
        }

        Ok(())
    }
}

#[async_trait]
impl Task for TitleGeneratorTask {
    fn name(&self) -> &'static str {
        "title_generator"
    }

    async fn start(
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
        if let Some(conversation) = ctx.get_conversation_mut(&self.conversation_id) {
            conversation.title = self.title;
        }

        Ok(())
    }
}
