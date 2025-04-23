use jp_conversation::{
    thread::{Document, Documents, Thinking, Thread},
    AssistantMessage, MessagePair, UserMessage,
};
use serde::Serialize;
use serde_json::Value;
use tracing::trace;

use super::{
    chat::{self, CacheControl, Content, Message, Transform},
    tool::{self, FunctionCall, Tool, ToolCall},
};
use crate::Error;

/// Chat completion request matching the `OpenRouter` API schema.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ChatCompletion {
    /// The model ID to use.
    pub model: String,

    /// The list of messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<RequestMessage>,

    /// Reasoning configuration.
    ///
    /// Should be `None` if the model does not support reasoning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,

    /// Tool calling field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,

    /// Message transforms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<Transform>,

    /// Stop words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,

    /// Whether to return log probabilities of the output tokens or not. If
    /// true, returns the log probabilities of each output token returned in the
    /// content of message.
    #[serde(default, skip_serializing_if = "logprobs_is_false")]
    pub logprobs: bool,
}

#[expect(clippy::trivially_copy_pass_by_ref)]
fn logprobs_is_false(logprobs: &bool) -> bool {
    !logprobs
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct Reasoning {
    pub exclude: bool,
    pub effort: ReasoningEffort,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    High,
    #[default]
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct RequestMessages(pub Vec<RequestMessage>);

impl TryFrom<Thread> for RequestMessages {
    type Error = Error;

    #[expect(clippy::too_many_lines)]
    fn try_from(thread: Thread) -> Result<Self, Self::Error> {
        #[expect(
            unused_variables,
            reason = "To be used when we add attachments and MCP tools"
        )]
        let Thread {
            conversation,
            model,
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
                for result in results {
                    messages.push(RequestMessage::Tool(tool::Message {
                        tool_call_id: result.id,
                        content: result.content,
                        name: None,
                    }));
                }
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase", tag = "role")]
pub enum RequestMessage {
    User(chat::Message),
    Assistant(chat::Message),
    System(chat::Message),
    Tool(tool::Message),
}

impl RequestMessage {
    #[must_use]
    pub fn content_mut(&mut self) -> &mut [chat::Content] {
        match self {
            Self::Assistant(m) | Self::User(m) | Self::System(m) => m.content.as_mut_slice(),
            Self::Tool(_) => &mut [],
        }
    }

    #[must_use]
    pub fn chat_message_mut(&mut self) -> Option<&mut chat::Message> {
        match self {
            Self::Assistant(m) | Self::User(m) | Self::System(m) => Some(m),
            Self::Tool(_) => None,
        }
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<RequestMessage> {
    let mut msgs = vec![];
    match msg.message {
        UserMessage::Query(query) if !query.is_empty() => {
            msgs.push(Message::default().with_text(query).user());
        }
        UserMessage::Query(_) => {}
        UserMessage::ToolCallResults(results) => {
            msgs.extend(results.into_iter().map(|result| {
                RequestMessage::Tool(tool::Message {
                    tool_call_id: result.id,
                    content: result.content,
                    name: None,
                })
            }));
        }
    }

    let AssistantMessage {
        reasoning,
        content,
        tool_calls,
    } = msg.reply;

    let mut message = Message::default();
    if let Some(content) = content {
        message = message.with_text(content);
    }
    if let Some(reasoning) = reasoning {
        message = message.with_reasoning(reasoning);
    }
    message.tool_calls = tool_calls
        .into_iter()
        .map(|tool_call| ToolCall::Function {
            id: Some(tool_call.id),
            index: 0,
            function: FunctionCall {
                name: Some(tool_call.name),
                arguments: match tool_call.arguments {
                    Value::Null => None,
                    v => serde_json::to_string(&v).ok(),
                },
            },
        })
        .collect();

    // Some LLM providers disallow empty messages, but sometimes an
    // LLM response can be empty, so we need to filter those out.
    if !message.content.is_empty() || !message.tool_calls.is_empty() {
        msgs.push(message.assistant());
    }

    msgs
}
