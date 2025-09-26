use std::error::Error;

use async_trait::async_trait;
use jp_config::{
    model::{
        id::ProviderId,
        parameters::{CustomReasoningConfig, ReasoningEffort},
        ModelConfig,
    },
    providers::llm::LlmProviderConfig,
    AppConfig,
};
use jp_conversation::{message::Messages, AssistantMessage, ConversationId, MessagePair};
use jp_llm::{provider, structured};
use jp_workspace::Workspace;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

use crate::Task;

#[derive(Debug)]
pub struct TitleGeneratorTask {
    pub conversation_id: ConversationId,
    pub model: ModelConfig,
    pub providers: LlmProviderConfig,
    pub messages: Messages,
    pub title: Option<String>,
}

impl TitleGeneratorTask {
    pub fn new(
        conversation_id: ConversationId,
        config: &AppConfig,
        workspace: &Workspace,
        query: Option<String>,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let mut messages = workspace.get_messages(&conversation_id).to_messages();
        if let Some(query) = query {
            messages.push(
                MessagePair::new(
                    query.into(),
                    // TODO: We have to use a placeholder here, because the
                    // assistant message is not yet generated, while some LLM
                    // providers (such as Anthropic) require a non-empty message to
                    // generate a response.
                    //
                    // Is there a better way to do this?
                    AssistantMessage::from((ProviderId::Anthropic, "<RESPONSE PENDING>")),
                ),
                None,
            );
        }

        // Prefer the title generation model id, otherwise use the assistant
        // model id.
        let mut model = config
            .conversation
            .title
            .generate
            .model
            .clone()
            .unwrap_or(ModelConfig {
                id: config.assistant.model.id.clone(),
                ..Default::default()
            });

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
            model,
            providers: config.providers.llm.clone(),
            messages,
            title: None,
        })
    }

    async fn update_title(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(conversation_id = %self.conversation_id, "Updating conversation title.");

        let parameters = &self.model.parameters;
        let provider_config = &self.providers;
        let model_id = &self.model.id;
        let provider_id = model_id.provider;

        let provider = provider::get_provider(provider_id, provider_config)?;
        let query = structured::titles::titles(1, self.messages.clone(), &[])?;
        let titles: Vec<String> =
            structured::completion(provider.as_ref(), model_id, parameters, query).await?;

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
        tokio::select! {
            () = token.cancelled() => {
                trace!(conversation_id = %id, "Title generator task cancelled.");
            }
            result = self.update_title() => match result {
                Ok(()) => trace!(conversation_id = %id, "Title generator task completed."),
                Err(error) => {
                    warn!(?error, conversation_id = %id, "Title generator task failed.");
                    return Err(error)
                }
            }
        };

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
