use std::env;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt as _};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::{ParametersConfig, ReasoningEffort},
    },
    providers::llm::openrouter::OpenrouterConfig,
};
use jp_conversation::{
    AssistantMessage, UserMessage,
    event::{ConversationEvent, EventKind},
    message::ToolCallRequest,
    thread::{Document, Documents, Thread},
};
use jp_openrouter::{
    Client,
    types::{
        chat::{CacheControl, Content, Message},
        request::{self, RequestMessage},
        response::{
            self, ChatCompletion as OpenRouterChunk, Choice, ErrorResponse, FinishReason,
            StreamingDelta,
        },
        tool::{self, FunctionCall, Tool, ToolCall, ToolFunction},
    },
};
use serde::Serialize;
use tracing::{debug, trace, warn};

use super::{CompletionChunk, Delta, Event, EventStream, ModelDetails, Reply, StreamEvent};
use crate::{
    Error,
    error::Result,
    provider::{Provider, openai::parameters_with_strict_mode},
    query::ChatQuery,
    stream::{accumulator::Accumulator, event::StreamEndReason},
};

static PROVIDER: ProviderId = ProviderId::Openrouter;

#[derive(Debug, Clone)]
pub struct Openrouter {
    client: Client,
}

impl Openrouter {
    fn new(api_key: String, app_name: Option<String>, app_referrer: Option<String>) -> Self {
        Self {
            client: Client::new(api_key, app_name, app_referrer),
        }
    }

    /// Set the base URL for the Openrouter API.
    fn with_base_url(mut self, base_url: String) -> Self {
        self.client = self.client.with_base_url(base_url);
        self
    }
}

#[async_trait]
impl Provider for Openrouter {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails> {
        let id: ModelIdConfig = (PROVIDER, name.as_ref()).try_into()?;

        Ok(self
            .models()
            .await?
            .into_iter()
            .find(|m| m.id == id)
            .unwrap_or(ModelDetails::empty(id)))
    }

    async fn models(&self) -> Result<Vec<ModelDetails>> {
        self.client
            .models()
            .await?
            .data
            .into_iter()
            .map(map_model)
            .collect::<Result<_>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<EventStream> {
        debug!(
            model = %model.id,
            "Starting OpenRouter chat completion stream."
        );

        let request = build_request(query, model, parameters)?;
        let inner_stream = self
            .client
            .chat_completion_stream(request)
            .map_err(Error::from);

        #[expect(clippy::semicolon_if_nothing_returned)]
        Ok(Box::pin(try_stream!({
            let mut accumulator = Accumulator::new(200);
            tokio::pin!(inner_stream);

            while let Some(result) = inner_stream.next().await {
                let chunk = match result {
                    Ok(chunk) => chunk,
                    Err(e) => {
                        warn!(error = ?e, "Error receiving delta from OpenRouter stream.");
                        Err(e)?
                    }
                };

                trace!(?chunk, "Received OpenRouter delta.");

                let choice_data = chunk.choices.into_iter().next();
                let Some(choice) = choice_data else {
                    trace!("OpenRouter delta had no choices, skipping.");
                    continue;
                };

                let Choice::Streaming(streaming_choice) = choice else {
                    warn!("Received non-streaming choice in streaming context, ignoring.");
                    continue;
                };

                let finish_reason = streaming_choice.finish_reason;

                let mut delta: Delta = streaming_choice.delta.into();
                delta.tool_call_finished = streaming_choice
                    .finish_reason
                    .is_some_and(|r| matches!(r, FinishReason::ToolCalls | FinishReason::Stop));

                for event in delta.into_stream_events(&mut accumulator)? {
                    yield event;
                }

                if let Some(finish_reason) = finish_reason {
                    for event in accumulator.drain()? {
                        yield event;
                    }

                    match finish_reason {
                        FinishReason::Length => {
                            yield StreamEvent::EndOfStream(StreamEndReason::MaxTokens)
                        }
                        FinishReason::Stop => {
                            yield StreamEvent::EndOfStream(StreamEndReason::Completed)
                        }
                        _ => {
                            yield StreamEvent::EndOfStream(StreamEndReason::Other(
                                finish_reason.as_str().to_owned(),
                            ))
                        }
                    }
                }
            }
        })))
    }

    async fn chat_completion(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<Reply> {
        let request = build_request(query, model, parameters)?;
        let completion =
            self.client.chat_completion(request).await.inspect_err(
                |error| warn!(%error, "Error receiving completion from OpenRouter."),
            )?;

        trace!(?completion, "Received OpenRouter delta.");

        let choice_data = completion.choices.into_iter().next();
        let Some(choice) = choice_data else {
            trace!("OpenRouter delta had no choices, skipping.");
            return Ok(Reply::default());
        };

        let Choice::NonStreaming(choice) = choice else {
            warn!("Received streaming choice in non-streaming context, ignoring.");
            return Ok(Reply::default());
        };

        if let Some(ErrorResponse { code, message, .. }) = choice.error {
            return Err(Error::InvalidResponse(format!(
                "OpenRouter error: {code}: {message}"
            )));
        }

        let mut events = vec![];
        if let Some(content) = choice.message.reasoning {
            events.push(Event::Reasoning(content));
        }
        if let Some(content) = choice.message.content {
            events.push(Event::Content(content));
        }
        for ToolCall::Function { function, id, .. } in choice.message.tool_calls {
            events.push(Event::ToolCall(ToolCallRequest {
                id: id.unwrap_or_default(),
                name: function.name.unwrap_or_default(),
                arguments: serde_json::from_str(&function.arguments.unwrap_or_default())
                    .unwrap_or(serde_json::Map::new()),
            }));
        }

        match choice.finish_reason {
            FinishReason::Length => events.push(Event::Finished(StreamEndReason::MaxTokens)),
            FinishReason::Stop => events.push(Event::Finished(StreamEndReason::Completed)),
            finish_reason => events.push(Event::Finished(StreamEndReason::Other(
                finish_reason.as_str().to_owned(),
            ))),
        }

        Ok(Reply {
            provider: PROVIDER,
            events,
        })
    }
}

/// Build request for Openrouter API.
fn build_request(
    query: ChatQuery,
    model: &ModelDetails,
    parameters: &ParametersConfig,
) -> Result<request::ChatCompletion> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let slug = model.id.name.to_string();
    let reasoning = model.custom_reasoning_config(parameters.reasoning);

    let messages: RequestMessages = (&model.id, thread).try_into()?;
    let tools = tools
        .into_iter()
        .map(|tool| Tool::Function {
            function: ToolFunction {
                parameters: parameters_with_strict_mode(tool.parameters, tool_call_strict_mode),
                name: tool.name,
                description: tool.description,
                strict: tool_call_strict_mode,
            },
        })
        .collect::<Vec<_>>();
    let tool_choice: tool::ToolChoice = if tools.is_empty() {
        tool::ToolChoice::None
    } else {
        match tool_choice {
            ToolChoice::Auto => tool::ToolChoice::Auto,
            ToolChoice::None => tool::ToolChoice::None,
            ToolChoice::Required => tool::ToolChoice::Required,
            ToolChoice::Function(name) => tool::ToolChoice::function(name),
        }
    };

    trace!(
        slug,
        messages_size = messages.0.len(),
        tools_size = tools.len(),
        "Built Openrouter request."
    );

    Ok(request::ChatCompletion {
        model: slug,
        messages: messages.0,
        reasoning: reasoning.map(|r| request::Reasoning {
            exclude: r.exclude,
            effort: match r.effort.abs_to_rel(model.max_output_tokens) {
                ReasoningEffort::High => request::ReasoningEffort::High,
                ReasoningEffort::Auto | ReasoningEffort::Medium => request::ReasoningEffort::Medium,
                ReasoningEffort::Low => request::ReasoningEffort::Low,
                ReasoningEffort::Absolute(_) => {
                    debug_assert!(false, "Reasoning effort must be relative.");
                    request::ReasoningEffort::Medium
                }
            },
        }),
        tools,
        tool_choice,
        ..Default::default()
    })
}

// TODO: Manually add a bunch of often-used models.
fn map_model(model: response::Model) -> Result<ModelDetails> {
    Ok(ModelDetails {
        id: (PROVIDER, model.id).try_into()?,
        display_name: Some(model.name),
        context_window: Some(model.context_length),
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: Some(model.created.date()),
        deprecated: None,
        features: vec![],
    })
}

impl From<StreamingDelta> for Delta {
    fn from(delta: StreamingDelta) -> Self {
        let tool_call = delta.tool_calls.into_iter().next();

        Self {
            content: delta.content,
            reasoning: delta.reasoning,
            tool_call_id: tool_call.as_ref().and_then(ToolCall::id),
            tool_call_name: tool_call.as_ref().and_then(ToolCall::name),
            tool_call_arguments: tool_call.as_ref().and_then(ToolCall::arguments),
            tool_call_finished: false,
        }
    }
}

impl TryFrom<&OpenrouterConfig> for Openrouter {
    type Error = Error;

    fn try_from(config: &OpenrouterConfig) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        let client = Openrouter::new(
            api_key,
            Some(config.app_name.clone()),
            config.app_referrer.clone(),
        )
        .with_base_url(config.base_url.clone());

        Ok(client)
    }
}

impl From<OpenRouterChunk> for CompletionChunk {
    fn from(chunk: OpenRouterChunk) -> Self {
        let reasoning = chunk
            .choices
            .first()
            .and_then(|choice| choice.reasoning().map(ToOwned::to_owned));

        if let Some(reasoning) = reasoning {
            return Self::Reasoning(reasoning);
        }

        let content = chunk
            .choices
            .first()
            .and_then(|choice| choice.content().map(ToOwned::to_owned))
            .unwrap_or_default();

        Self::Content(content)
    }
}

impl From<OpenRouterChunk> for StreamEvent {
    fn from(chunk: OpenRouterChunk) -> Self {
        StreamEvent::ChatChunk(chunk.into())
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct RequestMessages(pub Vec<RequestMessage>);

impl TryFrom<(&ModelIdConfig, Thread)> for RequestMessages {
    type Error = Error;

    #[expect(clippy::too_many_lines)]
    fn try_from((model_id, thread): (&ModelIdConfig, Thread)) -> Result<Self> {
        let Thread {
            system_prompt,
            instructions,
            attachments,
            mut history,
            message,
        } = thread;

        // If the last history message is a tool call response, we need to go
        // one more back in history, to avoid disjointing tool call requests and
        // their responses.
        let mut history_after_instructions = vec![];
        while let Some(event) = history.pop() {
            let tool_call_results = matches!(
                event.kind,
                EventKind::UserMessage(UserMessage::ToolCallResults(_))
            );
            history_after_instructions.insert(0, event);

            if !tool_call_results {
                break;
            }
        }

        let mut messages = vec![];
        let mut history = history
            .into_iter()
            .flat_map(event_to_messages)
            .collect::<Vec<_>>();

        // System message first, if any.
        //
        // Cached (1/4), as it's not expected to change.
        if let Some(system_prompt) = system_prompt {
            messages.push(
                Message::default()
                    .with_text(&system_prompt)
                    .with_cache()
                    .system(),
            );
        }

        // Historical messages second, these are static.
        //
        // Make sure to add cache control (2/4) to the last history message.
        if let Some(message) = history.last_mut().and_then(|m| m.chat_message_mut()) {
            message.cached();
        }
        messages.extend(history);

        // Group multiple contents blocks into a single message.
        let mut content = vec![];

        if !instructions.is_empty() {
            content.push(Content::Text {
                text: "Before we continue, here are some contextual details that will help you \
                       generate a better response."
                    .to_string(),
                cache_control: None,
            });
        }

        // Then instructions in XML tags.
        //
        // Cached (3/4), (for the last instruction), as it's not expected to
        // change.
        let mut instructions = instructions.iter().peekable();
        while let Some(instruction) = instructions.next() {
            content.push(Content::Text {
                text: instruction.try_to_xml()?,
                cache_control: instructions
                    .peek()
                    .map_or(Some(CacheControl::Ephemeral), |_| None),
            });
        }

        // Then large list of attachments, formatted as XML.
        //
        // see: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/long-context-tips>
        // see: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/use-xml-tags>
        //
        // Cached (4/4), more likely to change, but we'll keep the previous
        // cache if changed.
        if !attachments.is_empty() {
            let documents: Documents = attachments
                .into_iter()
                .enumerate()
                .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            content.push(Content::Text {
                text: documents.try_to_xml()?,
                cache_control: Some(CacheControl::Ephemeral),
            });
        }

        // Attach all data, and add a "fake" acknowledgement by the assistant.
        //
        // We insert the contextual data _before_ the last message pair, so that
        // there is a correct flow between the user and assistant when the
        // assistant requests a tool call.
        //
        // For example instead of:
        //
        // - ... history ...
        // - U: <user query>
        // - A: <tool call request>
        // - U: <instructions, attachments, etc...>
        // - A: Thank you for those details, ...
        // - U: <tool call response>
        //
        // We want:
        //
        // - ... history ...
        // - U: <instructions, attachments, etc...>
        // - A: Thank you for those details, ...
        // - U: <user query>
        // - A: <tool call request>
        // - U: <tool call response>
        if !content.is_empty() {
            messages.push(Message::default().with_content(content).user());
            messages.push(
                Message::default()
                    .with_text(
                        "Thank you for those details, I'll use them to inform my next response.",
                    )
                    .assistant(),
            );
        }

        messages.extend(
            history_after_instructions
                .into_iter()
                .flat_map(event_to_messages),
        );

        // User query
        match message {
            UserMessage::Query { query } => {
                messages.push(Message::default().with_text(query).user());
            }
            UserMessage::ToolCallResults(results) => {
                messages.extend(results.into_iter().map(|result| {
                    RequestMessage::Tool(tool::Message {
                        tool_call_id: result.id,
                        content: result.content,
                        name: None,
                    })
                }));
            }
        }

        // Only Anthropic and Google models support explicit caching.
        if !model_id.name.starts_with("anthropic") && !model_id.name.starts_with("google") {
            trace!(
                slug = %model_id.name,
                "Model does not support caching directives, disabling cache."
            );
            for m in &mut messages {
                m.content_mut().iter_mut().for_each(Content::disable_cache);
            }
        }

        Ok(RequestMessages(messages))
    }
}

fn event_to_messages(event: ConversationEvent) -> Vec<RequestMessage> {
    match event.kind {
        EventKind::UserMessage(user) => user_message_to_messages(user),
        EventKind::AssistantMessage(assistant) => {
            vec![assistant_message_to_message(assistant)]
        }
        EventKind::ConfigDelta(_) => vec![],
    }
}

fn user_message_to_messages(user: UserMessage) -> Vec<RequestMessage> {
    match user {
        UserMessage::Query { query } if !query.is_empty() => {
            vec![Message::default().with_text(query).user()]
        }
        UserMessage::Query { .. } => vec![],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| {
                RequestMessage::Tool(tool::Message {
                    tool_call_id: result.id,
                    content: result.content,
                    name: None,
                })
            })
            .collect(),
    }
}

fn assistant_message_to_message(assistant: AssistantMessage) -> RequestMessage {
    let AssistantMessage {
        provider: _,
        metadata: _,
        reasoning,
        content,
        tool_calls,
    } = assistant;

    let mut message = Message::default();
    if let Some(content) = content {
        message = message.with_text(content);
    }
    if let Some(reasoning) = reasoning {
        message = message.with_reasoning(reasoning);
    }
    message.tool_calls = tool_calls
        .into_iter()
        .map(|call| ToolCall::Function {
            id: Some(call.id),
            index: 0,
            function: FunctionCall {
                name: Some(call.name),
                arguments: if call.arguments.is_empty() {
                    None
                } else {
                    serde_json::to_string(&call.arguments).ok()
                },
            },
        })
        .collect();

    if message.content.is_empty() && message.tool_calls.is_empty() {
        message.content = vec![Content::text("<no response>")];
    }

    message.assistant()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jp_config::providers::llm::LlmProviderConfig;
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr() -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new("https://openrouter.ai", fixtures)
    }

    #[test(tokio::test)]
    async fn test_openrouter_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().openrouter;
        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = url;
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Openrouter::try_from(&config)
                    .unwrap()
                    .models()
                    .await
                    .map(|mut v| {
                        v.truncate(2);
                        v
                    })
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_openrouter_chat_completion() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let mut config = LlmProviderConfig::default().openrouter;
        let model_id = "openrouter/openai/o4-mini".parse().unwrap();
        let model = ModelDetails::empty(model_id);
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = url;
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Openrouter::try_from(&config)
                    .unwrap()
                    .chat_completion(&model, &ParametersConfig::default(), query)
                    .await
            },
        )
        .await
    }
}
