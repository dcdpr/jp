// pub mod anthropic;
// pub mod deepseek;
// pub mod google;
// pub mod xai;
pub mod openai;
pub mod openrouter;

use std::pin::Pin;

use futures::Stream;
use jp_config::llm::{self, provider};
use jp_conversation::{message::ToolCallRequest, model::ProviderId, thread::Thread};
use jp_mcp::Tool;
use openai::Openai;
use openrouter::Openrouter;
use tracing::warn;

use crate::{error::Result, Error};

/// Represents an event yielded by the chat completion stream.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of chat content or reasoning.
    ChatChunk(CompletionChunk),

    /// A request to call a tool.
    ToolCall(ToolCallRequest),
}

/// A chunk of chat content or reasoning.
#[derive(Debug, Clone)]
pub enum CompletionChunk {
    /// Regular chat content.
    Content(String),

    /// Reasoning content.
    Reasoning(String),
}

pub trait Provider {
    /// Perform a streaming chat completion.
    fn chat_completion_stream(
        &self,
        config: &llm::Config,
        thread: Thread,
        tools: Vec<Tool>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>>;
}

pub fn get_provider(id: ProviderId, config: &provider::Config) -> Result<Box<dyn Provider>> {
    let provider: Box<dyn Provider> = match id {
        // ProviderId::Anthropic => Box::new(Anthropic::try_from(&config.anthropic)?),
        // ProviderId::Deepseek => Box::new(Deepseek::try_from(&config.deepseek)?),
        // ProviderId::Google => Box::new(Google::try_from(&config.google)?),
        // ProviderId::Xai => Box::new(Xai::try_from(&config.xai)?),
        ProviderId::Openai => Box::new(Openai::try_from(&config.openai)?),
        ProviderId::Openrouter => Box::new(Openrouter::try_from(&config.openrouter)?),
        _ => todo!(),
    };

    Ok(provider)
}

struct Delta {
    content: Option<String>,
    reasoning: Option<String>,
    tool_call_id: Option<String>,
    tool_call_name: Option<String>,
    tool_call_arguments: Option<String>,
    tool_call_finished: bool,
}

// State for accumulating function calls.
#[derive(Default)]
enum AccumulationState {
    #[default]
    Idle,
    AccumulatingFunctionCall {
        id: String,
        name: String,
        arguments_buffer: String,
    },
}

impl AccumulationState {
    fn is_accumulating(&self) -> bool {
        matches!(self, Self::AccumulatingFunctionCall { .. })
    }
}

fn handle_delta(delta: Delta, state: &mut AccumulationState) -> Result<Option<StreamEvent>> {
    let Delta {
        content,
        reasoning,
        tool_call_id,
        tool_call_name,
        tool_call_arguments,
        tool_call_finished,
    } = delta;

    let reasoning = reasoning.and_then(|v| {
        if v.trim_matches(' ').is_empty() {
            None
        } else {
            Some(v)
        }
    });
    let content = content.and_then(|v| {
        if v.trim_matches(' ').is_empty() {
            None
        } else {
            Some(v)
        }
    });

    // Check for function call start or continuation.
    match state {
        AccumulationState::Idle => match tool_call_name {
            Some(name) => {
                *state = AccumulationState::AccumulatingFunctionCall {
                    id: tool_call_id.unwrap_or_default(),
                    name,
                    arguments_buffer: tool_call_arguments.unwrap_or_default(),
                };
            }
            None if tool_call_arguments.is_some() => {
                return Err(Error::Other(
                    "Received function call arguments without a function name.".into(),
                ));
            }
            _ => {}
        },
        AccumulationState::AccumulatingFunctionCall {
            arguments_buffer, ..
        } => {
            if let Some(args_chunk) = tool_call_arguments {
                arguments_buffer.push_str(&args_chunk);
            }
        }
    }

    // Check for function call completion.
    if tool_call_finished {
        let AccumulationState::AccumulatingFunctionCall {
            id,
            name,
            arguments_buffer,
        } = state
        else {
            warn!("Received tool_calls finish reason but was not accumulating a function call.");
            return Ok(None);
        };

        let id = id.clone();
        let name = name.clone();
        let arguments = match serde_json::from_str(arguments_buffer) {
            Ok(arguments) => arguments,
            Err(e) => {
                return Err(Error::Other(format!(
                    "Failed to parse function call arguments: {e}. Buffer was: \
                     '{arguments_buffer}'"
                )));
            }
        };

        *state = AccumulationState::default();
        return Ok(Some(StreamEvent::ToolCall(ToolCallRequest {
            id,
            name,
            arguments,
        })));
    }

    // Handle reasoning.
    if let Some(reasoning) = reasoning {
        if !state.is_accumulating() {
            return Ok(Some(StreamEvent::ChatChunk(CompletionChunk::Reasoning(
                reasoning,
            ))));
        }

        warn!(
            reasoning,
            "Ignoring reasoning chunk while accumulating function call."
        );
    }

    // Handle regular content.
    if let Some(content) = content {
        if !state.is_accumulating() {
            return Ok(Some(StreamEvent::ChatChunk(CompletionChunk::Content(
                content,
            ))));
        }

        warn!(
            content_len = content.len(),
            "Ignoring content chunk while accumulating function call."
        );
    }

    Ok(None)
}
