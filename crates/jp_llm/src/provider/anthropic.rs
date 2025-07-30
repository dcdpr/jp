use std::env;

use async_anthropic::{
    messages::DEFAULT_MAX_TOKENS,
    types::{
        self, ListModelsResponse, Thinking, ToolBash, ToolCodeExecution, ToolComputerUse,
        ToolTextEditor, ToolWebSearch,
    },
    Client,
};
use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use jp_config::{assistant, model::parameters::Parameters};
use jp_conversation::{
    event::{ConversationEvent, EventKind},
    thread::{Document, Documents, Thread},
    AssistantMessage, UserMessage,
};
use jp_mcp::tool;
use jp_model::{ModelId, ProviderId};
use jp_query::query::ChatQuery;
use serde_json::Value;
use time::macros::date;
use tracing::{info, trace, warn};

use super::{Event, EventStream, ModelDetails, Provider, ReasoningDetails, Reply, StreamEvent};
use crate::{
    error::{Error, Result},
    provider::{handle_delta, AccumulationState, Delta},
};

#[derive(Debug, Clone)]
pub struct Anthropic {
    client: Client,
}

#[async_trait]
impl Provider for Anthropic {
    async fn models(&self) -> Result<Vec<ModelDetails>> {
        let (mut after_id, mut models) = self
            .client
            .models()
            .list()
            .await
            .map(|list| (list.has_more.then_some(list.last_id).flatten(), list.data))?;

        while let Some(id) = &after_id {
            let (id, data) = self
                .client
                .get::<ListModelsResponse>(&format!("/v1/models?after_id={id}"))
                .await
                .map(|list| (list.has_more.then_some(list.last_id).flatten(), list.data))?;

            models.extend(data);
            after_id = id;
        }

        Ok(models.into_iter().map(map_model).collect())
    }

    async fn chat_completion(
        &self,
        model: &ModelId,
        parameters: &Parameters,
        query: ChatQuery,
    ) -> Result<Reply> {
        let details = self.models().await?;
        let model_details = get_details_for_model(model, &details);
        let request = create_request(model, model_details, parameters, query)?;

        self.client
            .messages()
            .create(request)
            .await
            .map_err(Into::into)
            .and_then(map_response)
            .map(Reply)
    }

    async fn chat_completion_stream(
        &self,
        model_id: &ModelId,
        parameters: &Parameters,
        query: ChatQuery,
    ) -> Result<EventStream> {
        let client = self.client.clone();
        let details = self.models().await?;
        let model_details = get_details_for_model(model_id, &details);
        let request = create_request(model_id, model_details, parameters, query)?;

        Ok(Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            let stream = client
                .messages()
                .create_stream(request).await
                .map_err(|e| match e {
                    async_anthropic::errors::AnthropicError::RateLimit { retry_after } =>
                        Error::RateLimit { retry_after },
                    _ => Error::from(e),
                });

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                for event in map_event(event?, &mut current_state) {
                    yield event;
                }
            }
        }))
    }
}

fn create_request(
    model_id: &ModelId,
    model_details: Option<&ModelDetails>,
    parameters: &Parameters,
    query: ChatQuery,
) -> Result<types::CreateMessagesRequest> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let mut builder = types::CreateMessagesRequestBuilder::default();
    let system_prompt = thread.system_prompt.clone();

    builder
        .model(model_id.slug())
        .messages(convert_thread(thread)?);

    if let Some(system_prompt) = system_prompt {
        builder.system(types::Text {
            text: system_prompt,
            cache_control: Some(types::CacheControl::default()),
        });
    }

    let tools = convert_tools(tools, tool_call_strict_mode);
    let tool_choice_function = matches!(tool_choice, jp_mcp::tool::ToolChoice::Function(_));
    let tool_choice = convert_tool_choice(tool_choice);
    if !tools.is_empty() {
        builder.tools(tools).tool_choice(tool_choice);
    }

    let max_tokens = parameters
        .max_tokens
        .or_else(|| model_details.as_ref().and_then(|d| d.max_output_tokens))
        .unwrap_or_else(|| {
            warn!(
                %model_id,
                %DEFAULT_MAX_TOKENS,
                "Model `max_tokens` parameter not found, using default value."
            );

            DEFAULT_MAX_TOKENS as u32
        });

    if let Some(thinking) = parameters.reasoning {
        let (supported, min_supported, max_supported) = if tool_choice_function {
            info!(
                "Anthropic API does not support reasoning when tool_choice forces tool use. \
                 Disabling reasoning."
            );
            (false, 0, None)
        } else if let Some(details) = model_details.as_ref().and_then(|d| d.reasoning) {
            (details.supported, details.min_tokens, details.max_tokens)
        } else {
            warn!(
                %model_id,
                "Model reasoning support unknown, but the request requested it. This may \
            result in unexpected behavior"
            );

            (true, 0, None)
        };

        if supported {
            builder.thinking(types::ExtendedThinking {
                kind: "enabled".to_string(),
                budget_tokens: thinking
                    .effort
                    .to_tokens(max_tokens)
                    .max(min_supported)
                    .min(max_supported.unwrap_or(u32::MAX)),
            });
        } else {
            warn!(
                %model_id,
                "Model does not support reasoning, but the request requested it. Reasnoning \
                 disabled."
            );
        }
    }

    if let Some(temperature) = parameters.temperature {
        builder.temperature(temperature);
    }

    #[expect(clippy::cast_possible_wrap)]
    builder.max_tokens(max_tokens as i32);

    if let Some(top_p) = parameters.top_p {
        builder.top_p(top_p);
    }

    if let Some(top_k) = parameters.top_k {
        builder.top_k(top_k);
    }

    builder.build().map_err(Into::into)
}

fn get_details_for_model<'a>(
    model_id: &ModelId,
    details: &'a [ModelDetails],
) -> Option<&'a ModelDetails> {
    // see: <https://docs.anthropic.com/en/docs/about-claude/models/overview#model-aliases>
    let details_slug = match model_id.slug() {
        "claude-opus-4-0" => "claude-opus-4-20250514",
        "claude-sonnet-4-0" => "claude-sonnet-4-20250514",
        "claude-3-7-sonnet-latest" => "claude-3-7-sonnet-20250219",
        "claude-3-5-sonnet-latest" => "claude-3-5-sonnet-20241022",
        "claude-3-5-haiku-latest" => "claude-3-5-haiku-20241022",
        slug => slug,
    };

    details.iter().find(|m| m.slug == details_slug)
}

fn map_model(model: types::Model) -> ModelDetails {
    match model.id.as_str() {
        "claude-opus-4-0" | "claude-opus-4-20250514" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(32_000),
            reasoning: Some(ReasoningDetails::supported()),
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
        },
        "claude-sonnet-4-0" | "claude-sonnet-4-20250514" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::supported()),
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
        },
        "claude-3-7-sonnet-latest" | "claude-3-7-sonnet-20250219" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::supported()),
            knowledge_cutoff: Some(date!(2024 - 11 - 1)),
        },
        "claude-3-5-haiku-latest" | "claude-3-5-haiku-20241022" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(8_192),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 7 - 1)),
        },
        "claude-3-5-sonnet-latest"
        | "claude-3-5-sonnet-20241022"
        | "claude-3-5-sonnet-20240620" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(8_192),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 4 - 1)),
        },
        "claude-3-opus-latest" | "claude-3-opus-20240229" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2023 - 8 - 1)),
        },
        "claude-3-haiku-20240307" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 8 - 1)),
        },
        id => {
            warn!(model = id, ?model, "Missing model details.");

            ModelDetails {
                provider: ProviderId::Anthropic,
                slug: model.id,
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            }
        }
    }
}

fn map_response(response: types::CreateMessagesResponse) -> Result<Vec<Event>> {
    response
        .content
        .into_iter()
        .filter_map(|item| {
            let metadata = match &item {
                types::MessageContent::Thinking(Thinking { signature, .. }) => {
                    signature.clone().map(|v| ("signature", v))
                }
                types::MessageContent::RedactedThinking { data } => {
                    Some(("redacted_thinking", data.clone()))
                }
                _ => None,
            };
            let v: Option<Result<Event>> = Delta::from(item).into();
            v.map(|v| (v, metadata))
        })
        .flat_map(|item| match item {
            (v, _) if v.is_err() => vec![v],
            (v, None) => vec![v],
            (v, Some((key, value))) => vec![v, Ok(Event::metadata(key, value))],
        })
        .collect::<Result<_>>()
}

fn map_event(
    event: types::MessagesStreamEvent,
    state: &mut AccumulationState,
) -> Vec<Result<StreamEvent>> {
    use types::{ContentBlockDelta::*, MessagesStreamEvent::*};

    match event {
        MessageStart { message, .. } => message
            .content
            .into_iter()
            .map(Delta::from)
            .filter_map(|v| handle_delta(v, state).transpose())
            .collect(),
        ContentBlockStart { content_block, .. } => handle_delta(content_block.into(), state)
            .transpose()
            .into_iter()
            .collect(),
        ContentBlockDelta { delta, .. } => match delta {
            TextDelta { text } => handle_delta(Delta::content(text), state)
                .transpose()
                .into_iter()
                .collect(),
            ThinkingDelta { thinking } => handle_delta(Delta::reasoning(thinking), state)
                .transpose()
                .into_iter()
                .collect(),
            InputJsonDelta { partial_json } => {
                handle_delta(Delta::tool_call("", "", partial_json), state)
                    .transpose()
                    .into_iter()
                    .collect()
            }

            // This is only used for thinking blocks, and we need to store this
            // signature to pass it back to the assistant in the message
            // history.
            //
            // See: <https://docs.anthropic.com/en/docs/build-with-claude/streaming#thinking-delta>
            SignatureDelta { signature } => vec![Ok(StreamEvent::metadata("signature", signature))],
        },
        MessageDelta { delta, .. }
            if delta.stop_reason.as_ref().is_some_and(|v| v == "tool_use") =>
        {
            handle_delta(Delta::tool_call_finished(), state)
                .transpose()
                .into_iter()
                .collect()
        }
        _ => vec![],
    }
}

impl TryFrom<&assistant::provider::anthropic::Anthropic> for Anthropic {
    type Error = Error;

    fn try_from(config: &assistant::provider::anthropic::Anthropic) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        Ok(Anthropic {
            client: Client::builder()
                .api_key(api_key)
                .base_url(config.base_url.clone())
                .beta("interleaved-thinking-2025-05-14,extended-cache-ttl-2025-04-11")
                .version("2023-06-01")
                .build()
                .map_err(|e| {
                    Error::Anthropic(async_anthropic::errors::AnthropicError::Unknown(
                        e.to_string(),
                    ))
                })?,
        })
    }
}

fn convert_tool_choice(choice: tool::ToolChoice) -> types::ToolChoice {
    match choice {
        tool::ToolChoice::None => types::ToolChoice::none(),
        tool::ToolChoice::Auto => types::ToolChoice::auto(),
        tool::ToolChoice::Required => types::ToolChoice::any(),
        tool::ToolChoice::Function(name) => types::ToolChoice::tool(name),
    }
}

fn convert_tools(tools: Vec<jp_mcp::Tool>, _strict: bool) -> Vec<types::Tool> {
    let mut tools: Vec<_> = tools
        .into_iter()
        .map(|tool| {
            types::Tool::Custom(types::CustomTool {
                name: tool.name.into(),
                description: tool.description.map(Into::into),
                input_schema: {
                    let mut map = tool.input_schema.as_ref().clone();
                    map.remove("type");

                    let required = map
                        .remove("required")
                        .map(|v| match v {
                            Value::Array(v) => v
                                .into_iter()
                                .filter_map(|v| match v {
                                    Value::String(v) => Some(v),
                                    _ => None,
                                })
                                .collect(),
                            _ => vec![],
                        })
                        .unwrap_or_default();

                    let properties = map
                        .remove("properties")
                        .map(|v| match v {
                            Value::Object(v) => v,
                            _ => serde_json::Map::default(),
                        })
                        .unwrap_or_default();

                    types::ToolInputSchema {
                        kind: types::ToolInputSchemaKind::Object,
                        properties,
                        required,
                    }
                },
                cache_control: None,
            })
        })
        .collect();

    // Cache tool definitions (4/4), as they are unlikely to change.
    if let Some(tool) = tools.last_mut() {
        let cache_control = match tool {
            types::Tool::Custom(tool) => &mut tool.cache_control,
            types::Tool::Bash(ToolBash::Bash20241022(tool)) => &mut tool.cache_control,
            types::Tool::Bash(ToolBash::Bash20250124(tool)) => &mut tool.cache_control,
            types::Tool::CodeExecution(ToolCodeExecution::CodeExecution20250522(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::ComputerUse(ToolComputerUse::ComputerUse20241022(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::ComputerUse(ToolComputerUse::ComputerUse20250124(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20241022(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20250124(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20250429(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::WebSearch(ToolWebSearch::WebSearch20250305(tool)) => {
                &mut tool.cache_control
            }
        };

        *cache_control = Some(types::CacheControl::default());
    }

    tools
}

fn convert_thread(thread: Thread) -> Result<Vec<types::Message>> {
    Messages::try_from(thread).map(|v| v.0)
}

struct Messages(Vec<types::Message>);

impl TryFrom<Thread> for Messages {
    type Error = Error;

    #[expect(clippy::too_many_lines)]
    fn try_from(thread: Thread) -> Result<Self> {
        let Thread {
            instructions,
            attachments,
            mut history,
            message,
            ..
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

        let mut items = vec![];
        let mut history = history
            .into_iter()
            .map(event_to_message)
            .collect::<Vec<_>>();

        // Historical messages second, these are static.
        //
        // Make sure to add cache control (1/4) to the last history message.
        if let Some(message) = history.last_mut().and_then(|m| m.content.0.last_mut()) {
            match message {
                types::MessageContent::Text(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                types::MessageContent::ToolUse(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                types::MessageContent::ToolResult(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                _ => {}
            }
        }
        items.extend(history);

        // Group multiple contents blocks into a single message.
        let mut content = vec![];

        if !instructions.is_empty() {
            content.push(
                "Before we continue, here are some contextual details that will help you generate \
                 a better response."
                    .into(),
            );
        }

        // Then instructions in XML tags.
        //
        // Cached (2/4), (for the last instruction), as it's not expected to
        // change.
        let mut instructions = instructions.iter().peekable();
        while let Some(instruction) = instructions.next() {
            content.push(types::MessageContent::Text(types::Text {
                text: instruction.try_to_xml()?,
                cache_control: instructions
                    .peek()
                    .map_or(Some(types::CacheControl::default()), |_| None),
            }));
        }

        // Then large list of attachments, formatted as XML.
        //
        // see: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/long-context-tips>
        // see: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/use-xml-tags>
        //
        // Cached (3/4), more likely to change, but we'll keep the previous
        // cache if changed.
        if !attachments.is_empty() {
            let documents: Documents = attachments
                .into_iter()
                .enumerate()
                .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            content.push(types::MessageContent::Text(types::Text {
                text: documents.try_to_xml()?,
                cache_control: Some(types::CacheControl::default()),
            }));
        }

        // Attach all data, and add a "fake" acknowledgement by the assistant.
        //
        // See `provider::openrouter` for more information.
        if !content.is_empty() {
            items.push(types::Message {
                role: types::MessageRole::User,
                content: types::MessageContentList(content),
            });
        }

        if items
            .last()
            .is_some_and(|m| matches!(m.role, types::MessageRole::User))
        {
            items.push(types::Message {
                role: types::MessageRole::Assistant,
                content: types::MessageContentList(vec![types::MessageContent::Text(
                    "Thank you for those details, I'll use them to inform my next response.".into(),
                )]),
            });
        }

        items.extend(history_after_instructions.into_iter().map(event_to_message));

        // User query
        match message {
            UserMessage::Query(text) => {
                items.push(types::Message {
                    role: types::MessageRole::User,
                    content: types::MessageContentList(vec![types::MessageContent::Text(
                        text.into(),
                    )]),
                });
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| types::Message {
                    role: types::MessageRole::User,
                    content: types::MessageContentList(vec![types::MessageContent::ToolResult(
                        types::ToolResult {
                            tool_use_id: result.id,
                            content: Some(result.content),
                            is_error: result.error,
                            cache_control: None,
                        },
                    )]),
                }));
            }
        }

        Ok(Self(items))
    }
}

fn event_to_message(event: ConversationEvent) -> types::Message {
    match event.kind {
        EventKind::UserMessage(user) => user_message_to_message(user),
        EventKind::AssistantMessage(assistant) => assistant_message_to_message(assistant),
    }
}

fn user_message_to_message(user: UserMessage) -> types::Message {
    let list = match user {
        UserMessage::Query(query) => vec![types::MessageContent::Text(query.into())],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| {
                types::MessageContent::ToolResult(types::ToolResult {
                    tool_use_id: result.id,
                    content: Some(result.content),
                    is_error: result.error,
                    cache_control: None,
                })
            })
            .collect(),
    };

    types::Message {
        role: types::MessageRole::User,
        content: types::MessageContentList(list),
    }
}

fn assistant_message_to_message(assistant: AssistantMessage) -> types::Message {
    let AssistantMessage {
        reasoning,
        content,
        tool_calls,
        metadata,
    } = assistant;

    let mut list = vec![];
    if let Some(data) = metadata
        .get("redacted_thinking")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        list.push(types::MessageContent::RedactedThinking { data });
    } else if let Some(thinking) = reasoning {
        let thinking = Thinking {
            thinking,
            signature: metadata
                .get("signature")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };

        list.push(types::MessageContent::Thinking(thinking));
    }

    if let Some(text) = content {
        list.push(types::MessageContent::Text(text.into()));
    }

    for tool_call in tool_calls {
        list.push(types::MessageContent::ToolUse(types::ToolUse {
            id: tool_call.id,
            input: tool_call.arguments,
            name: tool_call.name,
            cache_control: None,
        }));
    }

    types::Message {
        role: types::MessageRole::Assistant,
        content: types::MessageContentList(list),
    }
}

impl From<types::MessageContent> for Delta {
    fn from(item: types::MessageContent) -> Self {
        match item {
            types::MessageContent::Text(text) => Delta::content(text.text),
            types::MessageContent::Thinking(thinking) => Delta::reasoning(thinking.thinking),
            types::MessageContent::RedactedThinking { .. } => Delta::reasoning(String::new()),
            types::MessageContent::ToolUse(tool_use) => {
                Delta::tool_call(tool_use.id, tool_use.name, match &tool_use.input {
                    Value::Object(map) if !map.is_empty() => tool_use.input.to_string(),
                    _ => String::new(),
                })
            }
            types::MessageContent::ToolResult(_) => {
                debug_assert!(false, "Unexpected message content: {item:?}");
                Delta::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jp_config::{
        model::parameters::{Reasoning, ReasoningEffort},
        Configurable as _, Partial as _,
    };
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr() -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new("https://api.anthropic.com", fixtures)
    }

    #[test(tokio::test)]
    async fn test_anthropic_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config =
            assistant::Assistant::from_partial(assistant::AssistantPartial::default_values())
                .unwrap()
                .provider
                .anthropic;

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

                Anthropic::try_from(&config).unwrap().models().await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_chat_completion() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let mut config =
            assistant::Assistant::from_partial(assistant::AssistantPartial::default_values())
                .unwrap()
                .provider
                .anthropic;
        let model_id = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
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

                Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model_id, &Parameters::default(), query)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_redacted_thinking(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config =
            assistant::Assistant::from_partial(assistant::AssistantPartial::default_values())
                .unwrap()
                .provider
                .anthropic;
        let model_id = "anthropic/claude-3-7-sonnet-latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                // See: <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking#thinking-redaction>
                message: "ANTHROPIC_MAGIC_STRING_TRIGGER_REDACTED_THINKING_46C9A13E193C177646C7398A98432ECCCE4C1253D5E2D82641AC0E52CC2876CB".into(),
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

                let parameters = Parameters {
                    reasoning: Some(Reasoning {
                        effort: ReasoningEffort::Medium,
                        exclude: false,
                    }),
                    ..Default::default()
                };

                let events = Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model_id, &parameters, query.clone())
                    .await
                    .unwrap()
                    .into_inner();

                assert!(events.iter().any(
                    |event| matches!(event, Event::Metadata(k, _) if k == "redacted_thinking")
                ));

                events
            },
        )
        .await
    }

    #[test]
    fn test_create_request() {
        let model_id = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let parameters = Parameters {
            reasoning: Some(Reasoning {
                effort: ReasoningEffort::Medium,
                exclude: false,
            }),
            top_p: Some(1.0),
            top_k: Some(40),
            ..Default::default()
        };

        let model_details = map_model(types::Model {
            id: "claude-3-5-haiku-latest".to_owned(),
            display_name: String::new(),
            created_at: String::new(),
            model_type: String::new(),
        });

        let request = create_request(&model_id, Some(&model_details), &parameters, query);

        insta::assert_debug_snapshot!(request);
    }

    #[test]
    fn test_get_details_for_model() {
        let details = vec![
            ModelDetails {
                provider: ProviderId::Anthropic,
                slug: "claude-opus-4-20250514".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
            ModelDetails {
                provider: ProviderId::Anthropic,
                slug: "claude-sonnet-4-20250514".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
            ModelDetails {
                provider: ProviderId::Anthropic,
                slug: "claude-3-7-sonnet-20250219".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
            ModelDetails {
                provider: ProviderId::Anthropic,
                slug: "claude-3-5-sonnet-20241022".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
            ModelDetails {
                provider: ProviderId::Anthropic,
                slug: "claude-3-5-haiku-20241022".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
        ];

        let cases = vec![
            ("anthropic/claude-opus-4-0", Some(&details[0])),
            ("anthropic/claude-opus-4-20250514", Some(&details[0])),
            ("anthropic/claude-sonnet-4-0", Some(&details[1])),
            ("anthropic/claude-sonnet-4-20250514", Some(&details[1])),
            ("anthropic/claude-3-7-sonnet-latest", Some(&details[2])),
            ("anthropic/claude-3-7-sonnet-20250219", Some(&details[2])),
            ("anthropic/claude-3-5-sonnet-latest", Some(&details[3])),
            ("anthropic/claude-3-5-sonnet-20241022", Some(&details[3])),
            ("anthropic/claude-3-5-haiku-latest", Some(&details[4])),
            ("anthropic/claude-3-5-haiku-20241022", Some(&details[4])),
            ("anthropic/nonexistent", None),
        ];

        for (model_id, expected) in cases {
            let actual = get_details_for_model(&model_id.parse().unwrap(), &details);
            assert_eq!(actual, expected);
        }
    }
}
