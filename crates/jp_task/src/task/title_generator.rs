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
use jp_conversation::{
    ConversationEvent, ConversationId, ConversationStream,
    event::{ChatRequest, ChatResponse},
    event_builder::EventBuilder,
    thread::ThreadBuilder,
};
use jp_llm::{
    event::Event,
    provider,
    retry::{RetryConfig, collect_with_retry},
    title,
};
use jp_workspace::Workspace;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

use crate::Task;

#[derive(Debug)]
pub struct TitleGeneratorTask {
    pub conversation_id: ConversationId,
    pub model_id: ModelIdConfig,
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
        // otherwise limit it to low effort.
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
            providers: config.providers.llm.clone(),
            events,
            title: None,
        })
    }

    async fn update_title(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!(conversation_id = %self.conversation_id, "Updating conversation title.");

        let provider = provider::get_provider(self.model_id.provider, &self.providers)?;
        let model = provider.model_details(&self.model_id.name).await?;

        let sections = title::title_instructions(1, &[]);
        let thread = ThreadBuilder::default()
            .with_events(self.events.clone())
            .with_sections(sections)
            .build()?;

        let schema = title::title_schema(1);
        let mut events = thread.events.clone();
        events.add_chat_request(ChatRequest {
            content: "Generate a title for this conversation.".into(),
            schema: Some(schema),
        });

        let query = jp_llm::query::ChatQuery {
            thread: jp_conversation::thread::Thread { events, ..thread },
            tools: vec![],
            tool_choice: jp_config::assistant::tool_choice::ToolChoice::default(),
        };

        let retry_config = RetryConfig::default();
        let llm_events =
            collect_with_retry(provider.as_ref(), &model, query, &retry_config).await?;

        // Pipe raw streaming events through the EventBuilder so that
        // structured JSON chunks are concatenated and parsed into a
        // proper Value (rather than individual Value::String fragments).
        let mut builder = EventBuilder::new();
        let mut flushed = Vec::new();
        for event in llm_events {
            match event {
                Event::Part { index, event } => builder.handle_part(index, event),
                Event::Flush { index, metadata } => {
                    flushed.extend(builder.handle_flush(index, metadata));
                }
                Event::Finished(_) => flushed.extend(builder.drain()),
            }
        }

        let structured_data = flushed
            .into_iter()
            .filter_map(ConversationEvent::into_chat_response)
            .find_map(ChatResponse::into_structured_data);

        if let Some(data) = structured_data {
            let titles = title::extract_titles(&data);
            trace!(titles = ?titles, "Received conversation titles.");
            if let Some(t) = titles.into_iter().next() {
                self.title = Some(t);
            }
        } else {
            warn!(conversation_id = %self.conversation_id, "No structured data in title response.");
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
        if let Some(mut conversation) = ctx.get_conversation_mut(&self.conversation_id) {
            conversation.title = self.title;
        }

        Ok(())
    }
}
