use std::env;

use async_anthropic::{
    messages::DEFAULT_MAX_TOKENS,
    types::{self, ListModelsResponse, Thinking},
    Client,
};
use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use jp_config::llm;
use jp_conversation::{
    model::ProviderId,
    thread::{Document, Documents, Thread},
    AssistantMessage, MessagePair, Model, UserMessage,
};
use jp_mcp::tool;
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

impl Anthropic {
    async fn create_request(
        &self,
        model: &Model,
        query: ChatQuery,
    ) -> Result<types::CreateMessagesRequest> {
        let ChatQuery {
            thread,
            tools,
            tool_choice,
            tool_call_strict_mode,
        } = query;

        let details = self
            .models()
            .await?
            .into_iter()
            .find(|m| m.slug == model.id.slug());

        let mut builder = types::CreateMessagesRequestBuilder::default();
        let system_prompt = thread.system_prompt.clone();

        builder
            .model(model.id.slug())
            .messages(convert_thread(thread)?);

        if let Some(system_prompt) = system_prompt {
            builder.system(system_prompt);
        }

        let tools = convert_tools(tools, tool_call_strict_mode);
        let tool_choice_function = matches!(tool_choice, jp_mcp::tool::ToolChoice::Function(_));
        let tool_choice = convert_tool_choice(tool_choice);
        if !tools.is_empty() {
            builder.tools(tools).tool_choice(tool_choice);
        }

        let max_tokens = model
            .parameters
            .max_tokens
            .or_else(|| details.as_ref().and_then(|d| d.max_output_tokens))
            .unwrap_or(DEFAULT_MAX_TOKENS as u32);

        if let Some(thinking) = model.parameters.reasoning {
            let (supported, min_supported, max_supported) = if tool_choice_function {
                info!(
                    "Anthropic API does not support reasoning when tool_choice forces tool use. \
                     Disabling reasoning."
                );
                (false, 0, None)
            } else if let Some(details) = details.as_ref().and_then(|d| d.reasoning) {
                (details.supported, details.min_tokens, details.max_tokens)
            } else {
                warn!(
                    %model.id,
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
                    %model.id,
                    "Model does not support reasoning, but the request requested it. Reasnoning \
                     disabled."
                );
            }
        }

        if let Some(temperature) = model.parameters.temperature {
            builder.temperature(temperature);
        }

        #[expect(clippy::cast_possible_wrap)]
        builder.max_tokens(max_tokens as i32);

        if let Some(top_p) = model.parameters.top_p {
            builder.top_p(top_p);
        }

        if let Some(top_k) = model.parameters.top_k {
            builder.top_k(top_k);
        }

        builder.build().map_err(Into::into)
    }
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

    async fn chat_completion(&self, model: &Model, query: ChatQuery) -> Result<Reply> {
        let request = self.create_request(model, query).await?;
        self.client
            .messages()
            .create(request)
            .await
            .map_err(Into::into)
            .and_then(map_response)
            .map(Reply)
    }

    async fn chat_completion_stream(&self, model: &Model, query: ChatQuery) -> Result<EventStream> {
        let client = self.client.clone();
        let request = self.create_request(model, query).await?;
        let stream = Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            let stream = client
                .messages()
                .create_stream(request).await
                .map_err(Error::from);

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                for event in map_event(event?, &mut current_state) {
                    yield event;
                }
            }
        });

        Ok(stream)
    }
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
        .flatten()
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

impl TryFrom<&llm::provider::anthropic::Config> for Anthropic {
    type Error = Error;

    fn try_from(config: &llm::provider::anthropic::Config) -> Result<Self> {
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

fn convert_tools(tools: Vec<jp_mcp::Tool>, _strict: bool) -> Vec<serde_json::Map<String, Value>> {
    tools
        .into_iter()
        .map(|tool| {
            let mut map = serde_json::Map::new();
            map.insert("name".to_owned(), tool.name.into());
            map.insert("description".to_owned(), tool.description.into());
            map.insert(
                "input_schema".to_owned(),
                tool.input_schema.as_ref().clone().into(),
            );

            map
        })
        .collect()
}

fn convert_thread(thread: Thread) -> Result<Vec<types::Message>> {
    Messages::try_from(thread).map(|v| v.0)
}

struct Messages(Vec<types::Message>);

impl TryFrom<Thread> for Messages {
    type Error = Error;

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
        while let Some(message) = history.pop() {
            let tool_call_results = matches!(message.message, UserMessage::ToolCallResults(_));
            history_after_instructions.insert(0, message);

            if !tool_call_results {
                break;
            }
        }

        let mut items = vec![];
        let history = history
            .into_iter()
            .flat_map(message_pair_to_messages)
            .collect::<Vec<_>>();

        // Historical messages second, these are static.
        items.extend(history);

        // Group multiple contents blocks into a single message.
        let mut content = vec![];

        if !instructions.is_empty() {
            content.push(
                "Before we continue, here are some contextual details that will help you generate \
                 a better response."
                    .to_string(),
            );
        }

        // Then instructions in XML tags.
        for instruction in &instructions {
            content.push(instruction.try_to_xml()?);
        }

        // Then large list of attachments, formatted as XML.
        if !attachments.is_empty() {
            let documents: Documents = attachments
                .into_iter()
                .enumerate()
                .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            content.push(documents.try_to_xml()?);
        }

        // Attach all data, and add a "fake" acknowledgement by the assistant.
        //
        // See `provider::openrouter` for more information.
        if !content.is_empty() {
            items.push(types::Message {
                role: types::MessageRole::User,
                content: types::MessageContentList(
                    content
                        .into_iter()
                        .map(|s| types::MessageContent::Text(s.into()))
                        .collect(),
                ),
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

        items.extend(
            history_after_instructions
                .into_iter()
                .flat_map(message_pair_to_messages),
        );

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
                        },
                    )]),
                }));
            }
        }

        Ok(Self(items))
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<types::Message> {
    let (user, assistant) = msg.split();

    vec![
        user_message_to_message(user),
        assistant_message_to_message(assistant),
    ]
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

    use jp_conversation::{
        model::{Reasoning, ReasoningEffort},
        ModelId,
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
        let mut config = llm::Config::default().provider.anthropic;
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
        let mut config = llm::Config::default().provider.anthropic;
        let model: ModelId = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
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
                    .chat_completion(&model.into(), query)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_redacted_thinking(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.anthropic;
        let model: ModelId = "anthropic/claude-3-7-sonnet-latest".parse().unwrap();
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

                let mut model: Model = model.into();
                model.parameters.reasoning = Some(Reasoning {
                    effort: ReasoningEffort::Medium,
                    exclude: false,
                });

                let events = Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model, query.clone())
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
}
