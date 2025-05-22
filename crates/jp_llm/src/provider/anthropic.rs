use std::env;

use async_anthropic::{
    types::{self, ListModelsResponse},
    Client,
};
use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use jp_config::llm;
use jp_conversation::{
    model::ProviderId,
    thread::{Document, Documents, Thinking, Thread},
    AssistantMessage, MessagePair, Model, UserMessage,
};
use jp_query::query::ChatQuery;
use serde_json::Value;
use time::macros::date;
use tracing::{trace, warn};

use super::{Event, EventStream, ModelDetails, Provider, StreamEvent};
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

    async fn chat_completion(&self, model: &Model, query: ChatQuery) -> Result<Vec<Event>> {
        let client = self.client.clone();
        let request = create_request(model, query)?;
        client
            .messages()
            .create(request)
            .await
            .map_err(Into::into)
            .and_then(map_response)
    }

    fn chat_completion_stream(&self, model: &Model, query: ChatQuery) -> Result<EventStream> {
        let client = self.client.clone();
        let request = create_request(model, query)?;
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
        "claude-3-7-sonnet-latest" | "claude-3-7-sonnet-20250219" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(true),
            knowledge_cutoff: Some(date!(2024 - 11 - 1)),
        },
        "claude-3-5-haiku-latest" | "claude-3-5-haiku-20241022" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(8_192),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2024 - 7 - 1)),
        },
        "claude-3-5-sonnet-latest"
        | "claude-3-5-sonnet-20241022"
        | "claude-3-5-sonnet-20240620" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(8_192),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2024 - 4 - 1)),
        },
        "claude-3-opus-latest" | "claude-3-opus-20240229" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2023 - 8 - 1)),
        },
        "claude-3-haiku-20240307" => ModelDetails {
            provider: ProviderId::Anthropic,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(false),
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
        .filter_map(|item| Delta::from(item).into())
        .collect::<Result<Vec<_>>>()
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
            InputJsonDelta { partial_json } => {
                handle_delta(Delta::tool_call("", "", partial_json), state)
                    .transpose()
                    .into_iter()
                    .collect()
            }
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

fn create_request(model: &Model, query: ChatQuery) -> Result<types::CreateMessagesRequest> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let mut builder = types::CreateMessagesRequestBuilder::default();
    let system_prompt = thread.system_prompt.clone();

    builder
        .model(model.id.slug())
        .messages(convert_thread(thread)?);

    if let Some(system_prompt) = system_prompt {
        builder.system(system_prompt);
    }

    let tools = convert_tools(tools, tool_call_strict_mode);
    if !tools.is_empty() {
        builder
            .tools(tools)
            .tool_choice(convert_tool_choice(tool_choice));
    }

    if let Some(temperature) = model.parameters.get("temperature").and_then(Value::as_f64) {
        #[expect(clippy::cast_possible_truncation)]
        builder.temperature(temperature as f32);
    }

    if let Some(max_tokens) = model.parameters.get("max_tokens").and_then(Value::as_u64) {
        #[expect(clippy::cast_possible_truncation)]
        builder.max_tokens(max_tokens as i32);
    }

    if let Some(top_p) = model.parameters.get("top_p").and_then(Value::as_f64) {
        #[expect(clippy::cast_possible_truncation)]
        builder.top_p(top_p as f32);
    }

    if let Some(top_k) = model.parameters.get("top_k").and_then(Value::as_u64) {
        #[expect(clippy::cast_possible_truncation)]
        builder.top_k(top_k as u32);
    }

    builder.build().map_err(Into::into)
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

fn convert_tool_choice(choice: llm::ToolChoice) -> types::ToolChoice {
    match choice {
        llm::ToolChoice::Auto | llm::ToolChoice::None => types::ToolChoice::Auto,
        llm::ToolChoice::Required => types::ToolChoice::Any,
        llm::ToolChoice::Function(name) => types::ToolChoice::Tool(name),
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

    #[expect(clippy::too_many_lines)]
    fn try_from(thread: Thread) -> Result<Self> {
        let Thread {
            instructions,
            attachments,
            mut history,
            reasoning,
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
                    role: types::MessageRole::Assistant,
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

        // Reasoning message last, in `<thinking>` tags.
        if let Some(content) = reasoning {
            items.push(types::Message {
                role: types::MessageRole::Assistant,
                content: types::MessageContentList(vec![types::MessageContent::Text(
                    Thinking(content).try_to_xml()?.into(),
                )]),
            });
        }

        Ok(Self(items))
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<types::Message> {
    let (user, assistant) = msg.split();

    user_message_to_messages(user)
        .into_iter()
        .chain(assistant_message_to_messages(assistant))
        .collect()
}

fn user_message_to_messages(user: UserMessage) -> Vec<types::Message> {
    match user {
        UserMessage::Query(query) if !query.is_empty() => vec![types::Message {
            role: types::MessageRole::User,
            content: types::MessageContentList(vec![types::MessageContent::Text(query.into())]),
        }],
        UserMessage::Query(_) => vec![],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| types::Message {
                role: types::MessageRole::User,
                content: types::MessageContentList(vec![types::MessageContent::ToolResult(
                    types::ToolResult {
                        tool_use_id: result.id,
                        content: Some(result.content),
                        is_error: result.error,
                    },
                )]),
            })
            .collect(),
    }
}

fn assistant_message_to_messages(assistant: AssistantMessage) -> Vec<types::Message> {
    let AssistantMessage {
        content,
        tool_calls,
        ..
    } = assistant;

    let mut items = vec![];
    if let Some(text) = content {
        items.push(types::Message {
            role: types::MessageRole::Assistant,
            content: types::MessageContentList(vec![types::MessageContent::Text(text.into())]),
        });
    }

    for tool_call in tool_calls {
        items.push(types::Message {
            role: types::MessageRole::Assistant,
            content: types::MessageContentList(vec![types::MessageContent::ToolUse(
                types::ToolUse {
                    id: tool_call.id,
                    input: tool_call.arguments,
                    name: tool_call.name,
                },
            )]),
        });
    }

    items
}

impl From<types::MessageContent> for Delta {
    fn from(item: types::MessageContent) -> Self {
        match item {
            types::MessageContent::Text(text) => Delta::content(text.text),
            types::MessageContent::ToolUse(tool_use) => {
                Delta::tool_call(tool_use.id, tool_use.name, tool_use.input.to_string())
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

    use jp_conversation::ModelId;
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

                Anthropic::try_from(&config)
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
                    .map(|mut v| {
                        v.truncate(10);
                        v
                    })
            },
        )
        .await
    }
}
