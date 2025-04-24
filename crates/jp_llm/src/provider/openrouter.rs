use std::{env, pin::Pin};

use async_stream::stream;
use futures::{Stream, StreamExt, TryStreamExt as _};
use jp_config::llm::{self, provider::openrouter};
use jp_conversation::{
    model::ReasoningEffort,
    thread::{Document, Documents, Thinking, Thread},
    AssistantMessage, MessagePair, UserMessage,
};
use jp_openrouter::{
    types::{
        chat::{CacheControl, Content, Message},
        request::{self, RequestMessage},
        response::{ChatCompletion as OpenRouterChunk, Choice, FinishReason, StreamingDelta},
        tool::{self, FunctionCall, Tool, ToolCall, ToolFunction},
    },
    Client,
};
use serde::Serialize;
use serde_json::Value;
use tracing::{debug, trace, warn};

use super::{CompletionChunk, Delta, StreamEvent};
use crate::{
    error::Result,
    provider::{handle_delta, AccumulationState, Provider},
    Error,
};

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

    /// Build request for Openrouter API.
    fn build_request(thread: Thread, tools: Vec<jp_mcp::Tool>) -> Result<request::ChatCompletion> {
        let slug = thread.model.slug.clone();
        let reasoning = thread.model.reasoning;
        let messages: RequestMessages = thread.try_into()?;
        let tools = tools
            .into_iter()
            .map(|tool| Tool::Function {
                function: ToolFunction {
                    name: tool.name.to_string(),
                    description: tool.description.map(|v| v.to_string()),
                    parameters: tool.input_schema.as_ref().clone().into_iter().collect(),
                },
            })
            .collect::<Vec<_>>();

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
                effort: match r.effort {
                    ReasoningEffort::High => request::ReasoningEffort::High,
                    ReasoningEffort::Medium => request::ReasoningEffort::Medium,
                    ReasoningEffort::Low => request::ReasoningEffort::Low,
                },
            }),
            tools,
            ..Default::default()
        })
    }
}

impl Provider for Openrouter {
    fn chat_completion_stream(
        &self,
        _config: &llm::Config,
        thread: Thread,
        tools: Vec<jp_mcp::Tool>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        debug!(
            model = thread.model.slug,
            "Starting OpenRouter chat completion stream."
        );

        let request = Self::build_request(thread, tools)?;
        let inner_stream = self
            .client
            .chat_completion_stream(&request)
            .map_err(Error::from);

        let stream = Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            tokio::pin!(inner_stream);

            while let Some(result) = inner_stream.next().await {
                let chunk = match result {
                    Ok(chunk) => chunk,
                    Err(e) => {
                        warn!(error = ?e, "Error receiving delta from OpenRouter stream.");
                        yield Err(e);
                        continue
                    }
                };

                trace!(?chunk, "Received OpenRouter delta.");

                let choice_data = chunk.choices.into_iter().next();
                let Some(choice) = choice_data else {
                    trace!("OpenRouter delta had no choices, skipping.");
                    continue
                };

                let Choice::Streaming(streaming_choice) = choice else {
                    warn!("Received non-streaming choice in streaming context, ignoring.");
                    continue
                };

                let mut delta: Delta = streaming_choice.delta.into();
                delta.tool_call_finished = streaming_choice.finish_reason.is_some_and(|reason| matches!(reason, FinishReason::ToolCalls));

                match handle_delta(delta, &mut current_state) {
                    Ok(Some(event)) => yield Ok(event),
                    Ok(None) => {}
                    Err(error) => {
                        warn!(?error, "Error handling OpenRouter delta.");
                        yield Err(error);
                    }
                }
            }
        });

        Ok(stream)
    }
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

impl TryFrom<&openrouter::Config> for Openrouter {
    type Error = Error;

    fn try_from(config: &openrouter::Config) -> Result<Self> {
        Ok(Openrouter::new(
            env::var(&config.api_key_env)?,
            Some(config.app_name.clone()),
            config.app_referrer.clone(),
        ))
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

impl TryFrom<Thread> for RequestMessages {
    type Error = Error;

    #[expect(clippy::too_many_lines)]
    fn try_from(thread: Thread) -> Result<Self> {
        let Thread {
            model,
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
        let mut history = history
            .into_iter()
            .flat_map(message_pair_to_messages)
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
        let mut content = vec![Content::Text {
            text: "Before we continue, here are some contextual details that will help you \
                   generate a better response."
                .to_string(),
            cache_control: None,
        }];

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
        messages.push(Message::default().with_content(content).user());
        messages.push(
            Message::default()
                .with_text("Thank you for those details, I'll use them to inform my next response.")
                .assistant(),
        );
        messages.extend(
            history_after_instructions
                .into_iter()
                .flat_map(message_pair_to_messages),
        );

        // User query
        match message {
            UserMessage::Query(query) => {
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

        // Reasoning message last, in `<thinking>` tags.
        if let Some(content) = reasoning {
            messages.push(
                Message::default()
                    .with_text(Thinking(content).try_to_xml()?)
                    .assistant(),
            );
        }

        // Only Anthropic and Google models support explicit caching.
        if !model.slug.starts_with("anthropic") && !model.slug.starts_with("google") {
            trace!(
                slug = model.slug,
                "Model does not support caching directives, disabling cache."
            );

            messages
                .iter_mut()
                .for_each(|m| m.content_mut().iter_mut().for_each(Content::disable_cache));
        }

        Ok(RequestMessages(messages))
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<RequestMessage> {
    let (user, assistant) = msg.split();

    user_message_to_messages(user)
        .into_iter()
        .chain(Some(assistant_message_to_message(assistant)))
        .collect()
}

fn user_message_to_messages(user: UserMessage) -> Vec<RequestMessage> {
    match user {
        UserMessage::Query(query) if !query.is_empty() => {
            vec![Message::default().with_text(query).user()]
        }
        UserMessage::Query(_) => vec![],
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
                arguments: match call.arguments {
                    Value::Null => None,
                    v => serde_json::to_string(&v).ok(),
                },
            },
        })
        .collect();

    if message.content.is_empty() && message.tool_calls.is_empty() {
        message.content = vec![Content::text("<no response>")];
    }

    message.assistant()
}
