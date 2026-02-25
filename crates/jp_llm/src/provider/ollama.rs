use std::str::FromStr as _;

use async_trait::async_trait;
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _, future, stream};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ReasoningConfig,
    },
    providers::llm::ollama::OllamaConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatResponse, EventKind, ToolCallRequest},
};
use ollama_rs::{
    Ollama as Client,
    error::OllamaError,
    generation::{
        chat::{ChatMessage, ChatMessageResponse, MessageRole, request::ChatMessageRequest},
        parameters::{FormatType, JsonStructure, KeepAlive, TimeUnit},
        tools::{ToolCall, ToolCallFunction, ToolFunctionInfo, ToolInfo, ToolType},
    },
    models::{LocalModel, ModelOptions},
};
use serde_json::{Map, Value};
use tracing::{debug, trace};
use url::Url;

use super::{EventStream, ModelDetails, Provider};
use crate::{
    error::{Error, Result, StreamError},
    event::{Event, FinishReason},
    query::ChatQuery,
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Ollama;

#[derive(Debug, Clone)]
pub struct Ollama {
    client: Client,
}

#[async_trait]
impl Provider for Ollama {
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
        let models = self.client.list_local_models().await?;

        models.into_iter().map(map_model).collect::<Result<_>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream> {
        debug!(
            model = %model.id.name,
            "Starting Ollama chat completion stream."
        );

        let (request, is_structured) = create_request(model, query)?;

        trace!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            "Sending request to Ollama."
        );

        Ok(self
            .client
            .send_chat_messages_stream(request)
            .await?
            .map(|v| v.map_err(|()| StreamError::other("Ollama stream error")))
            .map_ok({
                let mut reasoning_flushed = false;
                move |v| {
                    stream::iter(
                        map_event(v, is_structured, &mut reasoning_flushed)
                            .into_iter()
                            .map(Ok),
                    )
                }
            })
            .try_flatten()
            .chain(future::ready(Ok(Event::Finished(FinishReason::Completed))).into_stream())
            .boxed())
    }
}

fn map_model(model: LocalModel) -> Result<ModelDetails> {
    Ok(ModelDetails {
        id: (PROVIDER, &model.name).try_into()?,
        display_name: Some(model.name),
        context_window: None,
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        features: vec![],
    })
}

/// Map an Ollama streaming chunk into provider-agnostic events.
///
/// Index convention: 0 = reasoning, 1 = message content, 2+ = tool calls.
///
/// Ollama guarantees that thinking tokens arrive before content/tool call
/// tokens. We exploit this by flushing the reasoning stream (index 0) as soon
/// as the first content or tool call chunk appears, ensuring the reasoning
/// event precedes content and tool call events in the history.
fn map_event(
    event: ChatMessageResponse,
    is_structured: bool,
    reasoning_flushed: &mut bool,
) -> Vec<Event> {
    let ChatMessageResponse { message, done, .. } = event;

    trace!(
        content = message.content,
        thinking = message.thinking,
        tool_calls = message.tool_calls.len(),
        done,
        "Ollama stream chunk."
    );

    let mut events = Vec::new();

    if let Some(thinking) = message.thinking
        && !thinking.is_empty()
    {
        events.push(Event::Part {
            index: 0,
            event: ConversationEvent::now(ChatResponse::reasoning(thinking)),
        });
    }

    let has_content = !message.content.is_empty();
    let has_tool_calls = !message.tool_calls.is_empty();

    // Flush reasoning before emitting content or tool calls so the reasoning
    // event always precedes them in the conversation history.
    if !*reasoning_flushed && (has_content || has_tool_calls) {
        events.push(Event::flush(0));
        *reasoning_flushed = true;
    }

    if has_content {
        let response = if is_structured {
            ChatResponse::structured(Value::String(message.content))
        } else {
            ChatResponse::message(message.content)
        };
        events.push(Event::Part {
            index: 1,
            event: ConversationEvent::now(response),
        });
    }

    for (
        index,
        ToolCall {
            function: ToolCallFunction { name, arguments },
        },
    ) in message.tool_calls.into_iter().enumerate()
    {
        let index = index + 2;
        events.push(Event::Part {
            index,
            event: ConversationEvent::now(ToolCallRequest {
                id: String::new(),
                name,
                arguments: match arguments {
                    Value::Object(map) => map,
                    v => Map::from_iter([("input".into(), v)]),
                },
            }),
        });
        events.push(Event::flush(index));
    }

    if done {
        if !*reasoning_flushed {
            events.push(Event::flush(0));
        }
        events.push(Event::flush(1));
    }

    events
}

fn create_request(model: &ModelDetails, query: ChatQuery) -> Result<(ChatMessageRequest, bool)> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
    } = query;

    let structured_schema = thread
        .events
        .last()
        .and_then(|e| e.event.as_chat_request())
        .and_then(|req| req.schema.clone())
        .map(|schema| JsonStructure::new_for_schema(schema.into()))
        .map(|schema| FormatType::StructuredJson(Box::new(schema)));

    let is_structured = structured_schema.is_some();

    let config = thread.events.config()?;
    let parameters = &config.assistant.model.parameters;

    let mut messages = thread.into_messages(to_system_messages, convert_events)?;

    if let Some(tool_choice) = tool_choice_to_system_message(&tool_choice) {
        messages.push(tool_choice);
    }

    let mut request = ChatMessageRequest::new(model.id.name.to_string(), messages);

    let tools = convert_tools(tools)?;
    if !tools.is_empty() {
        request = request.tools(tools);
    }

    let mut options = ModelOptions::default();

    if let Some(temperature) = parameters.temperature {
        options = options.temperature(temperature);
    }

    if let Some(top_p) = parameters.top_p {
        options = options.top_p(top_p);
    }

    if let Some(top_k) = parameters.top_k {
        options = options.top_k(top_k);
    }

    // Set the context window for the model.
    //
    // This can be used to force Ollama to use a larger context window then the
    // one determined based on the machine's resources.
    if let Some(context_window) = parameters
        .other
        .get("context_window")
        .and_then(Value::as_u64)
    {
        options = options.num_ctx(context_window);
    }

    if let Some(keep_alive) = parameters.other.get("keep_alive").and_then(Value::as_str) {
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

    // Reasoning for local models has to be explicitly enabled. This is because
    // there are too many models that do not support reasoning, and we have no
    // way (currently) to detect whether a model supports reasoning or not,
    // resulting in an error if the default reasoning of "auto" is used.
    if !matches!(parameters.reasoning, None | Some(ReasoningConfig::Off)) {
        request = request.think(true);
    }

    if let Some(schema) = structured_schema {
        request = request.format(schema);
    }

    Ok((request, is_structured))
}

impl TryFrom<&OllamaConfig> for Ollama {
    type Error = Error;

    fn try_from(config: &OllamaConfig) -> Result<Self> {
        let url = Url::from_str(&config.base_url)?;
        let port = url.port().unwrap_or(11434);
        let client = reqwest::Client::new();

        Ok(Ollama {
            client: Client::new_with_client(url, port, client),
        })
    }
}

fn convert_tools(tools: Vec<ToolDefinition>) -> Result<Vec<ToolInfo>> {
    tools
        .into_iter()
        .map(|tool| {
            Ok(ToolInfo {
                tool_type: ToolType::Function,
                function: ToolFunctionInfo {
                    parameters: tool.to_parameters_map().into(),
                    name: tool.name,
                    description: tool.description.unwrap_or_default(),
                },
            })
        })
        .collect::<Result<Vec<_>>>()
}

/// Poor-man's version of API-based tool choice. Needed until Ollama has
/// first-class support for tool choice.
fn tool_choice_to_system_message(choice: &ToolChoice) -> Option<ChatMessage> {
    let (ToolChoice::Function(_) | ToolChoice::Required) = choice else {
        return None;
    };

    let msg = if let Some(tool) = choice.function_name() {
        format!("You MUST use the function named '{tool}' available to you.")
    } else {
        "You MUST use AT LEAST ONE tool available to you.".to_string()
    };

    let content = format!(
        "IMPORTANT: {msg} DO NOT QUESTION THIS DIRECTIVE. DO NOT PROMPT FOR MORE CONTEXT OR \
         DETAILS. JUST RUN IT."
    );

    Some(ChatMessage {
        role: MessageRole::System,
        content,
        tool_calls: vec![],
        images: None,
        thinking: None,
    })
}

/// Convert some content into a system message.
fn to_system_messages(parts: Vec<String>) -> impl Iterator<Item = ChatMessage> {
    parts.into_iter().map(|content| ChatMessage {
        role: MessageRole::System,
        content,
        tool_calls: vec![],
        images: None,
        thinking: None,
    })
}

/// Convert a conversation stream into a list of messages.
fn convert_events(events: ConversationStream) -> Vec<ChatMessage> {
    events
        .into_iter()
        .filter_map(|event| match event.into_kind() {
            EventKind::ChatRequest(request) => Some(ChatMessage::user(request.content)),
            EventKind::ChatResponse(response) => match response {
                ChatResponse::Message { message } => Some(ChatMessage::assistant(message)),
                ChatResponse::Reasoning { reasoning, .. } => Some(ChatMessage {
                    role: MessageRole::Assistant,
                    content: String::new(),
                    tool_calls: vec![],
                    images: None,
                    thinking: Some(reasoning),
                }),
                ChatResponse::Structured { data } => Some(ChatMessage::assistant(data.to_string())),
            },
            EventKind::ToolCallRequest(request) => Some(ChatMessage {
                role: MessageRole::Assistant,
                content: String::new(),
                tool_calls: vec![ToolCall {
                    function: ToolCallFunction {
                        name: request.name,
                        arguments: Value::Object(request.arguments),
                    },
                }],
                images: None,
                thinking: None,
            }),
            EventKind::ToolCallResponse(response) => {
                Some(ChatMessage::tool(match response.result {
                    Ok(content) => content,
                    Err(error) => error,
                }))
            }
            _ => None,
        })
        .fold(vec![], |mut messages, message| match messages.last_mut() {
            Some(last)
                if last.role == message.role
                    && message.thinking.is_some()
                    && last.thinking.is_none() =>
            {
                last.thinking = message.thinking;
                messages
            }
            _ => {
                messages.push(message);
                messages
            }
        })
}

impl From<OllamaError> for StreamError {
    fn from(err: OllamaError) -> Self {
        use ollama_rs::error::InternalOllamaError;

        match err {
            OllamaError::ReqwestError(error) => Self::from(error),
            OllamaError::ToolCallError(error) => {
                StreamError::other("tool-call error").with_source(error)
            }
            OllamaError::JsonError(error) => StreamError::other("json error").with_source(error),
            OllamaError::InternalError(InternalOllamaError { message }) => {
                StreamError::other(message)
            }
            OllamaError::Other(message) => StreamError::other(message),
        }
    }
}
