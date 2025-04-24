use std::{env, pin::Pin};

use async_stream::stream;
use futures::Stream;
use jp_config::llm;
use jp_conversation::{
    thread::{Document, Documents, Thinking, Thread},
    AssistantMessage, MessagePair, UserMessage,
};
use jp_mcp::Tool;
use openai::{
    chat::{
        self, ChatCompletionBuilder, ChatCompletionChoiceDelta, ChatCompletionDelta,
        ChatCompletionFunctionDefinition, ChatCompletionGeneric, ChatCompletionMessage,
        ChatCompletionMessageDelta, ChatCompletionMessageRole, ToolCallFunction,
    },
    Credentials,
};
use serde::Serialize;
use tracing::{debug, trace, warn};

use super::{CompletionChunk, Delta, StreamEvent};
use crate::{
    error::{Error, Result},
    provider::{handle_delta, AccumulationState, Provider},
};

#[derive(Debug, Clone)]
pub struct Openai {
    credentials: Credentials,
}

impl Openai {
    fn new(api_key: String, base_url: String) -> Self {
        let credentials = Credentials::new(api_key, base_url);

        Self { credentials }
    }

    /// Build request for Openai API.
    fn build_request(&self, thread: Thread, tools: Vec<Tool>) -> Result<ChatCompletionBuilder> {
        let slug = thread.model.slug.clone();
        let messages: Messages = thread.try_into()?;
        let tools = tools
            .into_iter()
            .map(|tool| ChatCompletionFunctionDefinition {
                name: tool.name.to_string(),
                description: tool.description.map(|v| v.to_string()),
                parameters: Some(serde_json::Value::Object(
                    tool.input_schema.as_ref().clone(),
                )),
            })
            .collect::<Vec<_>>();

        trace!(
            slug,
            messages_size = messages.0.len(),
            tools_size = tools.len(),
            "Built Openai request."
        );

        Ok(ChatCompletionDelta::builder(&slug, messages.0)
            .credentials(self.credentials.clone())
            .functions(tools))
    }
}

impl Provider for Openai {
    fn chat_completion_stream(
        &self,
        _config: &llm::Config,
        thread: Thread,
        tools: Vec<Tool>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        debug!(
            model = thread.model.slug,
            "Starting Openai chat completion stream."
        );

        let request = self.build_request(thread, tools)?;
        let stream = Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            let stream = request
                .create_stream()
                .await
                .map_err(|e| Error::Other(format!("Failed to create Openai stream: {e}")))?;
            tokio::pin!(stream);

            while let Some(delta) = stream.recv().await {
                trace!(?delta, "Received delta.");

                let delta = delta.choices.into_iter().next().map(|c| (c.delta, c.finish_reason));
                let Some((delta, finish_reason)) = delta else {
                    continue
                };

                let mut delta: Delta = delta.into();
                delta.tool_call_finished = finish_reason.is_some_and(|reason| reason == "function_call");

                match handle_delta(delta, &mut current_state) {
                    Ok(Some(event)) => yield Ok(event),
                    Ok(None) => {}
                    Err(error) => {
                        warn!(?error, "Error handling OpenAI delta.");
                        yield Err(error);
                    }
                }
            }
        });

        Ok(stream)
    }
}

impl From<ChatCompletionMessageDelta> for Delta {
    fn from(delta: ChatCompletionMessageDelta) -> Self {
        let tool_call = delta.function_call.into_iter().next();

        Self {
            content: delta.content,
            reasoning: None,
            tool_call_id: delta.tool_call_id,
            tool_call_name: tool_call.as_ref().and_then(|call| call.name.clone()),
            tool_call_arguments: tool_call.as_ref().and_then(|call| call.arguments.clone()),
            tool_call_finished: false,
        }
    }
}

impl TryFrom<&llm::provider::openai::Config> for Openai {
    type Error = Error;

    fn try_from(config: &llm::provider::openai::Config) -> Result<Self> {
        let base_url = env::var(&config.base_url_env).unwrap_or(config.base_url.clone());

        Ok(Openai::new(env::var(&config.api_key_env)?, base_url))
    }
}

impl From<ChatCompletionGeneric<ChatCompletionChoiceDelta>> for CompletionChunk {
    fn from(chunk: ChatCompletionGeneric<ChatCompletionChoiceDelta>) -> Self {
        let content = chunk
            .choices
            .first()
            .and_then(|choice| choice.delta.content.as_deref().map(String::from))
            .unwrap_or_default();

        Self::Content(content)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Messages(pub Vec<ChatCompletionMessage>);

impl TryFrom<Thread> for Messages {
    type Error = Error;

    #[allow(clippy::too_many_lines)]
    fn try_from(thread: Thread) -> Result<Self> {
        let Thread {
            system_prompt,
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

        let mut messages = vec![];
        let history = history
            .into_iter()
            .flat_map(message_pair_to_messages)
            .collect::<Vec<_>>();

        // System message first, if any.
        if let Some(system_prompt) = system_prompt {
            messages.push(ChatCompletionMessage {
                role: ChatCompletionMessageRole::System,
                content: Some(system_prompt),
                ..Default::default()
            });
        }

        // Historical messages second, these are static.
        messages.extend(history);

        // Group multiple contents blocks into a single message.
        let mut content = vec![
            "Before we continue, here are some contextual details that will help you generate a \
             better response."
                .to_string(),
        ];

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
        messages.push(ChatCompletionMessage {
            role: ChatCompletionMessageRole::User,
            content: Some(content.join("\n\n")),
            ..Default::default()
        });
        messages.push(ChatCompletionMessage {
            role: ChatCompletionMessageRole::Assistant,
            content: Some(
                "Thank you for those details, I'll use them to inform my next response."
                    .to_string(),
            ),
            ..Default::default()
        });
        messages.extend(
            history_after_instructions
                .into_iter()
                .flat_map(message_pair_to_messages),
        );

        // User query
        match message {
            UserMessage::Query(query) => messages.push(ChatCompletionMessage {
                role: ChatCompletionMessageRole::User,
                content: Some(query),
                ..Default::default()
            }),
            UserMessage::ToolCallResults(results) => {
                messages.extend(results.into_iter().map(|result| ChatCompletionMessage {
                    role: ChatCompletionMessageRole::Tool,
                    tool_call_id: Some(result.id),
                    content: Some(result.content),
                    ..Default::default()
                }));
            }
        }

        // Reasoning message last, in `<thinking>` tags.
        if let Some(content) = reasoning {
            messages.push(ChatCompletionMessage {
                role: ChatCompletionMessageRole::Assistant,
                content: Some(Thinking(content).try_to_xml()?),
                ..Default::default()
            });
        }

        Ok(Messages(messages))
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<ChatCompletionMessage> {
    let (user, assistant) = msg.split();

    user_message_to_messages(user)
        .into_iter()
        .chain(Some(assistant_message_to_message(assistant)))
        .collect()
}

fn user_message_to_messages(user: UserMessage) -> Vec<ChatCompletionMessage> {
    match user {
        UserMessage::Query(query) if !query.is_empty() => vec![ChatCompletionMessage {
            role: ChatCompletionMessageRole::User,
            content: Some(query),
            ..Default::default()
        }],
        UserMessage::Query(_) => vec![],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| ChatCompletionMessage {
                role: ChatCompletionMessageRole::Tool,
                content: Some(result.content),
                tool_call_id: Some(result.id),
                ..Default::default()
            })
            .collect(),
    }
}

fn assistant_message_to_message(assistant: AssistantMessage) -> ChatCompletionMessage {
    let AssistantMessage {
        content,
        tool_calls,
        ..
    } = assistant;

    let mut message = ChatCompletionMessage {
        role: ChatCompletionMessageRole::Assistant,
        content,
        tool_calls: Some(
            tool_calls
                .into_iter()
                .map(|call| chat::ToolCall {
                    id: call.id,
                    r#type: "function".to_string(),
                    function: ToolCallFunction {
                        name: call.name,
                        arguments: call.arguments.to_string(),
                    },
                })
                .collect(),
        ),
        ..Default::default()
    };

    if message.content.as_ref().is_none_or(String::is_empty)
        && message.tool_calls.as_ref().is_none_or(Vec::is_empty)
    {
        message.content = Some("<no response>".to_owned());
    }

    message
}
