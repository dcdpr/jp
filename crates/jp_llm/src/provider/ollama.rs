use std::str::FromStr as _;

use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt as _;
use jp_config::llm;
use jp_conversation::{
    model::ProviderId,
    thread::{Document, Documents, Thinking, Thread},
    AssistantMessage, MessagePair, Model, UserMessage,
};
use jp_query::query::ChatQuery;
use ollama_rs::{
    generation::{
        chat::{request::ChatMessageRequest, ChatMessage, ChatMessageResponse, MessageRole},
        parameters::{KeepAlive, TimeUnit},
        tools::{ToolCall, ToolCallFunction, ToolFunctionInfo, ToolInfo, ToolType},
    },
    models::{LocalModel, ModelOptions},
    Ollama as Client,
};
use serde_json::Value;
use tracing::trace;
use url::Url;

use super::{handle_delta, Event, EventStream, ModelDetails, Provider, StreamEvent};
use crate::{
    error::{Error, Result},
    provider::{AccumulationState, Delta},
    CompletionChunk,
};

#[derive(Debug, Clone)]
pub struct Ollama {
    client: Client,
}

#[async_trait]
impl Provider for Ollama {
    async fn models(&self) -> Result<Vec<ModelDetails>> {
        let models = self.client.list_local_models().await?;

        Ok(models.into_iter().map(map_model).collect())
    }

    async fn chat_completion(&self, model: &Model, query: ChatQuery) -> Result<Vec<Event>> {
        let request = create_request(model, query)?;
        self.client
            .send_chat_messages(request)
            .await
            .map_err(Into::into)
            .and_then(map_response)
    }

    async fn chat_completion_stream(&self, model: &Model, query: ChatQuery) -> Result<EventStream> {
        let client = self.client.clone();
        let request = create_request(model, query)?;
        let stream = Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            let stream = client
                .send_chat_messages_stream(request.clone()).await
                .map_err(Error::from)?;

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                let events = event
                    .map(|event| map_event(event, &mut current_state))
                    .unwrap_or_default();

                for event in events {
                    yield event;
                }
            }
        });

        Ok(stream)
    }
}

fn map_model(model: LocalModel) -> ModelDetails {
    ModelDetails {
        provider: ProviderId::Ollama,
        slug: model.name,
        context_window: None,
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: None,
    }
}

fn map_response(response: ChatMessageResponse) -> Result<Vec<Event>> {
    map_event(response, &mut AccumulationState::default())
        .into_iter()
        .map(|v| {
            v.map(|e| match e {
                StreamEvent::ChatChunk(content) => match content {
                    CompletionChunk::Content(s) => Event::Content(s),
                    CompletionChunk::Reasoning(s) => Event::Reasoning(s),
                },
                StreamEvent::ToolCall(request) => Event::ToolCall(request),
            })
        })
        .collect::<Result<Vec<_>>>()
}

fn map_event(
    event: ChatMessageResponse,
    state: &mut AccumulationState,
) -> Vec<Result<StreamEvent>> {
    let mut events = vec![];

    for tool_call in event.message.tool_calls {
        let delta = Delta::tool_call(
            "",
            &tool_call.function.name,
            tool_call.function.arguments.to_string(),
        )
        .finished();

        events.extend(handle_delta(delta, state).transpose());
    }

    events.extend(handle_delta(Delta::content(event.message.content), state).transpose());

    events
}

fn create_request(model: &Model, query: ChatQuery) -> Result<ChatMessageRequest> {
    let ChatQuery {
        thread,
        tools,
        tool_choice: _,
        tool_call_strict_mode,
    } = query;

    let mut request = ChatMessageRequest::new(model.id.slug().to_owned(), convert_thread(thread)?);

    let tools = convert_tools(tools, tool_call_strict_mode)?;
    if !tools.is_empty() {
        request = request.tools(tools);
    }

    let mut options = ModelOptions::default();

    if let Some(temperature) = model.parameters.temperature {
        options = options.temperature(temperature);
    }

    if let Some(max_tokens) = model.parameters.max_tokens {
        options = options.num_ctx(max_tokens);
    }

    if let Some(top_p) = model.parameters.top_p {
        options = options.top_p(top_p);
    }

    if let Some(top_k) = model.parameters.top_k {
        options = options.top_k(top_k);
    }

    if let Some(keep_alive) = model
        .parameters
        .other
        .get("keep_alive")
        .and_then(Value::as_str)
    {
        let unit = keep_alive
            .chars()
            .last()
            .filter(char::is_ascii_alphabetic)
            .unwrap_or('m');

        let value = keep_alive
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>();

        request = request.keep_alive(KeepAlive::Until {
            time: value.parse::<u64>().unwrap_or(5),
            unit: match unit {
                's' => TimeUnit::Seconds,
                'h' => TimeUnit::Hours,
                _ => TimeUnit::Minutes,
            },
        });
    }

    request = request.options(options);

    Ok(request)
}

impl TryFrom<&llm::provider::ollama::Config> for Ollama {
    type Error = Error;

    fn try_from(config: &llm::provider::ollama::Config) -> Result<Self> {
        let url = Url::from_str(&config.base_url)?;
        let port = url.port().unwrap_or(11434);
        let client = reqwest::Client::new();

        Ok(Ollama {
            client: Client::new_with_client(url, port, client),
        })
    }
}

fn convert_tools(tools: Vec<jp_mcp::Tool>, _strict: bool) -> Result<Vec<ToolInfo>> {
    tools
        .into_iter()
        .map(|tool| {
            Ok(ToolInfo {
                tool_type: ToolType::Function,
                function: ToolFunctionInfo {
                    name: tool.name.to_string(),
                    description: tool.description.unwrap_or_default().to_string(),
                    parameters: serde_json::from_value(serde_json::Value::Object(
                        tool.input_schema.as_ref().clone(),
                    ))?,
                },
            })
        })
        .collect::<Result<Vec<_>>>()
}
//
fn convert_thread(thread: Thread) -> Result<Vec<ChatMessage>> {
    Messages::try_from(thread).map(|v| v.0)
}
struct Messages(Vec<ChatMessage>);

impl TryFrom<Thread> for Messages {
    type Error = Error;

    #[expect(clippy::too_many_lines)]
    fn try_from(thread: Thread) -> Result<Self> {
        let Thread {
            system_prompt,
            instructions,
            attachments,
            mut history,
            reasoning,
            message,
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

        // System message first, if any.
        if let Some(system_prompt) = system_prompt {
            items.push(ChatMessage {
                role: MessageRole::System,
                content: system_prompt,
                tool_calls: vec![],
                images: None,
            });
        }

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
            items.push(ChatMessage {
                role: MessageRole::User,
                content: content.join("\n\n"),
                tool_calls: vec![],
                images: None,
            });
        }

        if items
            .last()
            .is_some_and(|m| matches!(m.role, MessageRole::User))
        {
            items.push(ChatMessage {
                role: MessageRole::Assistant,
                content: "Thank you for those details, I'll use them to inform my next response."
                    .into(),
                tool_calls: vec![],
                images: None,
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
                items.push(ChatMessage {
                    role: MessageRole::User,
                    content: text,
                    tool_calls: vec![],
                    images: None,
                });
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| ChatMessage {
                    role: MessageRole::Tool,
                    content: result.content,
                    tool_calls: vec![],
                    images: None,
                }));
            }
        }

        // Reasoning message last, in `<thinking>` tags.
        if let Some(content) = reasoning {
            items.push(ChatMessage {
                role: MessageRole::Assistant,
                content: Thinking(content).try_to_xml()?,
                tool_calls: vec![],
                images: None,
            });
        }

        Ok(Self(items))
    }
}
fn message_pair_to_messages(msg: MessagePair) -> Vec<ChatMessage> {
    let (user, assistant) = msg.split();

    user_message_to_messages(user)
        .into_iter()
        .chain(assistant_message_to_messages(assistant))
        .collect()
}

fn user_message_to_messages(user: UserMessage) -> Vec<ChatMessage> {
    match user {
        UserMessage::Query(query) if !query.is_empty() => vec![ChatMessage {
            role: MessageRole::User,
            content: query,
            tool_calls: vec![],
            images: None,
        }],
        UserMessage::Query(_) => vec![],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| ChatMessage {
                role: MessageRole::Tool,
                content: result.content,
                tool_calls: vec![],
                images: None,
            })
            .collect(),
    }
}

fn assistant_message_to_messages(assistant: AssistantMessage) -> Vec<ChatMessage> {
    let AssistantMessage {
        content,
        tool_calls,
        ..
    } = assistant;

    let mut items = vec![];
    if let Some(text) = content {
        items.push(ChatMessage {
            role: MessageRole::Assistant,
            content: text,
            tool_calls: vec![],
            images: None,
        });
    }

    items.push(ChatMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        tool_calls: tool_calls
            .into_iter()
            .map(|call| ToolCall {
                function: ToolCallFunction {
                    name: call.name,
                    arguments: call.arguments,
                },
            })
            .collect(),
        images: None,
    });

    items
}

impl From<ToolCall> for Delta {
    fn from(item: ToolCall) -> Self {
        Delta::tool_call(
            &item.function.name,
            &item.function.name,
            item.function.arguments.to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, result::Result};

    use jp_conversation::ModelId;
    use jp_query::structured::conversation_titles;
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr(url: &str) -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new(url, fixtures)
    }

    #[test(tokio::test)]
    async fn test_ollama_models() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.ollama;
        let vcr = vcr(&config.base_url);
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;

                Ollama::try_from(&config)
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
    async fn test_ollama_chat_completion() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.ollama;
        let model: ModelId = "ollama/llama3:latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let vcr = vcr(&config.base_url);
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;

                Ollama::try_from(&config)
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

    #[test(tokio::test)]
    async fn test_ollama_chat_completion_stream() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.ollama;
        let model: ModelId = "ollama/llama3:latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let vcr = vcr(&config.base_url);
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;

                Ollama::try_from(&config)
                    .unwrap()
                    .chat_completion_stream(&model.into(), query)
                    .await
                    .unwrap()
                    .filter_map(
                        |r| async move { r.unwrap().into_chat_chunk().unwrap().into_content() },
                    )
                    .collect::<String>()
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_ollama_structured_completion() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.ollama;
        let model: ModelId = "ollama/llama3.1:8b".parse().unwrap();

        let message = UserMessage::Query("Test message".to_string());
        let history = vec![MessagePair::new(message, AssistantMessage::default())];

        let vcr = vcr(&config.base_url);
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;
                let query = conversation_titles(3, history, &[]).unwrap();

                Ollama::try_from(&config)
                    .unwrap()
                    .structured_completion(&model.into(), query)
                    .await
            },
        )
        .await
    }
}
