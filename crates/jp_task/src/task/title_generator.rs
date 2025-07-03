use std::error::Error;

use async_trait::async_trait;
use jp_config::{assistant::Assistant, Config};
use jp_conversation::{AssistantMessage, ConversationId, MessagePair};
use jp_llm::{provider, structured_completion};
use jp_model::ModelId;
use jp_query::structured::conversation_titles;
use jp_workspace::Workspace;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

use crate::Task;

#[derive(Debug)]
pub struct TitleGeneratorTask {
    pub conversation_id: ConversationId,
    pub model_id: ModelId,
    pub assistant: Assistant,
    pub messages: Vec<MessagePair>,
    pub title: Option<String>,
}

impl TitleGeneratorTask {
    pub fn new(
        conversation_id: ConversationId,
        config: &Config,
        workspace: &Workspace,
        query: Option<String>,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let mut messages = workspace.get_messages(&conversation_id).to_vec();
        if let Some(query) = query {
            messages.push(MessagePair::new(query.into(), AssistantMessage::default()));
        }

        let model_id = config
            .assistant
            .model
            .id
            .clone()
            .ok_or(jp_model::Error::MissingId)?;

        Ok(Self {
            conversation_id,
            model_id,
            assistant: config.assistant.clone(),
            messages,
            title: None,
        })
    }

    async fn update_title(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(conversation_id = %self.conversation_id, "Updating conversation title.");

        let parameters = &self.assistant.model.parameters;
        let provider_config = &self.assistant.provider;
        let model_id = &self.model_id;
        let provider_id = model_id.provider();

        let provider = provider::get_provider(provider_id, provider_config)?;
        let query = conversation_titles(1, self.messages.clone(), &[])?;
        let titles: Vec<String> =
            structured_completion(provider.as_ref(), model_id, parameters, query).await?;

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
