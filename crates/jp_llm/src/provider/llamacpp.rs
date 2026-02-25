use std::mem;

use async_trait::async_trait;
use futures::{StreamExt as _, future, stream};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::id::{ModelIdConfig, Name, ProviderId},
    providers::llm::llamacpp::LlamacppConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatResponse, EventKind, ToolCallResponse},
};
use reqwest_eventsource::{Event as SseEvent, EventSource};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, trace, warn};

use super::{
    EventStream, ModelDetails,
    openai::{ModelListResponse, ModelResponse, parameters_with_strict_mode},
};
use crate::{
    error::{Error, StreamError},
    event::{Event, FinishReason},
    provider::Provider,
    query::ChatQuery,
    stream::aggregator::{
        reasoning::ReasoningExtractor, tool_call_request::ToolCallRequestAggregator,
    },
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Llamacpp;

#[derive(Debug, Clone)]
pub struct Llamacpp {
    reqwest_client: reqwest::Client,
    base_url: String,
}

#[async_trait]
impl Provider for Llamacpp {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails, Error> {
        let id: ModelIdConfig = (PROVIDER, name.as_ref()).try_into()?;

        Ok(self
            .models()
            .await?
            .into_iter()
            .find(|m| m.id == id)
            .unwrap_or(ModelDetails::empty(id)))
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, Error> {
        self.reqwest_client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<ModelListResponse>()
            .await?
            .data
            .iter()
            .map(map_model)
            .collect::<Result<_, _>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream, Error> {
        debug!(
            model = %model.id.name,
            "Starting Llamacpp chat completion stream."
        );

        let (body, is_structured) = build_request(model, query)?;

        trace!(
            body = serde_json::to_string(&body).unwrap_or_default(),
            "Sending request to Llamacpp."
        );

        let request = self
            .reqwest_client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("content-type", "application/json")
            .json(&body);

        let es = EventSource::new(request).map_err(|e| Error::InvalidResponse(e.to_string()))?;

        let mut state = StreamState {
            extractor: ReasoningExtractor::default(),
            tool_calls: ToolCallRequestAggregator::default(),
            reasoning_flushed: false,
            finish_reason: None,
            is_structured,
        };

        Ok(es
            // EventSource yields Err on close; stop the stream.
            .take_while(|event| future::ready(event.is_ok()))
            .flat_map(move |event| stream::iter(handle_sse_event(event, &mut state)))
            .boxed())
    }
}

/// Mutable state carried across SSE events in a single stream.
struct StreamState {
    extractor: ReasoningExtractor,
    tool_calls: ToolCallRequestAggregator,
    reasoning_flushed: bool,
    /// Captured from `finish_reason` in the last choice delta. Emitted as
    /// `Event::Finished` when the `[DONE]` sentinel arrives.
    finish_reason: Option<FinishReason>,
    is_structured: bool,
}

/// Process a single SSE event into zero or more provider-agnostic events.
#[expect(clippy::too_many_lines)]
fn handle_sse_event(
    event: Result<SseEvent, reqwest_eventsource::Error>,
    state: &mut StreamState,
) -> Vec<Result<Event, StreamError>> {
    match event {
        Ok(SseEvent::Open) => vec![],
        Ok(SseEvent::Message(msg)) => {
            if msg.data == "[DONE]" {
                // Finalize the reasoning extractor on stream end.
                state.extractor.finalize();
                let mut events: Vec<Result<Event, StreamError>> =
                    drain_extractor(&mut state.extractor, state.is_structured)
                        .into_iter()
                        .map(Ok)
                        .collect();

                // Flush reasoning if we never did.
                if !state.reasoning_flushed {
                    events.push(Ok(Event::flush(0)));
                    state.reasoning_flushed = true;
                }

                // Flush message content.
                events.push(Ok(Event::flush(1)));

                events.push(Ok(Event::Finished(
                    state
                        .finish_reason
                        .take()
                        .unwrap_or(FinishReason::Completed),
                )));
                return events;
            }

            let chunk: StreamChunk = match serde_json::from_str(&msg.data) {
                Ok(c) => c,
                Err(error) => {
                    warn!(
                        error = error.to_string(),
                        data = &msg.data,
                        "Failed to parse Llamacpp chunk."
                    );

                    return vec![];
                }
            };

            let mut events = Vec::new();

            for choice in &chunk.choices {
                let delta = &choice.delta;

                // Reasoning via `reasoning_content` (deepseek / deepseek-legacy formats)
                if let Some(reasoning) = &delta.reasoning_content
                    && !reasoning.is_empty()
                {
                    events.push(Ok(Event::Part {
                        index: 0,
                        event: ConversationEvent::now(ChatResponse::reasoning(reasoning.clone())),
                    }));
                }

                // Content
                //
                // If reasoning_content was present, the server already
                // separated reasoning from content (deepseek /
                // deepseek-legacy). Otherwise, content may contain <think> tags
                // (none format) and needs the extractor.
                if let Some(content) = &delta.content
                    && !content.is_empty()
                {
                    // Server separated reasoning; content is pure text.
                    if delta.reasoning_content.is_some() {
                        flush_reasoning_if_needed(&mut events, &mut state.reasoning_flushed);

                        let response = if state.is_structured {
                            ChatResponse::structured(Value::String(content.clone()))
                        } else {
                            ChatResponse::message(content.clone())
                        };
                        events.push(Ok(Event::Part {
                            index: 1,
                            event: ConversationEvent::now(response),
                        }));
                    } else {
                        // Might contain <think> tags — feed through extractor.
                        state.extractor.handle(content);
                        events.extend(
                            drain_extractor(&mut state.extractor, state.is_structured)
                                .into_iter()
                                .map(Ok),
                        );
                    }
                }

                // Tool calls
                if let Some(tool_calls) = &delta.tool_calls {
                    flush_reasoning_if_needed(&mut events, &mut state.reasoning_flushed);

                    for tc in tool_calls {
                        let index = tc.index as usize + 2;
                        let name = tc.function.as_ref().and_then(|f| f.name.clone());
                        let arguments = tc.function.as_ref().and_then(|f| f.arguments.as_deref());
                        state
                            .tool_calls
                            .add_chunk(index, tc.id.clone(), name, arguments);
                    }
                }

                // Finish reason
                if let Some(reason) = &choice.finish_reason {
                    state.extractor.finalize();
                    events.extend(
                        drain_extractor(&mut state.extractor, state.is_structured)
                            .into_iter()
                            .map(Ok),
                    );

                    if matches!(reason.as_str(), "tool_calls" | "stop") {
                        events.extend(state.tool_calls.finalize_all().into_iter().flat_map(
                            |(index, result)| {
                                vec![
                                    result
                                        .map(|call| Event::Part {
                                            index,
                                            event: ConversationEvent::now(call),
                                        })
                                        .map_err(|e| StreamError::other(e.to_string())),
                                    Ok(Event::flush(index)),
                                ]
                            },
                        ));
                    }

                    if !state.reasoning_flushed {
                        events.push(Ok(Event::flush(0)));
                        state.reasoning_flushed = true;
                    }
                    events.push(Ok(Event::flush(1)));

                    // Per the OpenAI spec.
                    match reason.as_str() {
                        "length" => state.finish_reason = Some(FinishReason::MaxTokens),
                        "stop" => state.finish_reason = Some(FinishReason::Completed),
                        _ => {}
                    }
                }
            }

            events
        }
        Err(e) => vec![Err(StreamError::from(e))],
    }
}

/// Push a reasoning flush event if we haven't already.
fn flush_reasoning_if_needed(events: &mut Vec<Result<Event, StreamError>>, flushed: &mut bool) {
    if !*flushed {
        events.push(Ok(Event::flush(0)));
        *flushed = true;
    }
}

/// Drain accumulated content from the `ReasoningExtractor` into events.
///
/// Index convention matches Ollama: 0 = reasoning, 1 = message content.
fn drain_extractor(extractor: &mut ReasoningExtractor, is_structured: bool) -> Vec<Event> {
    let mut events = Vec::new();

    if !extractor.reasoning.is_empty() {
        let reasoning = mem::take(&mut extractor.reasoning);
        events.push(Event::Part {
            index: 0,
            event: ConversationEvent::now(ChatResponse::reasoning(reasoning)),
        });
    }

    if !extractor.other.is_empty() {
        let content = mem::take(&mut extractor.other);
        let response = if is_structured {
            ChatResponse::structured(Value::String(content))
        } else {
            ChatResponse::message(content)
        };
        events.push(Event::Part {
            index: 1,
            event: ConversationEvent::now(response),
        });
    }

    events
}

/// Build the JSON request body for the llama.cpp `/v1/chat/completions`
/// endpoint.
///
/// Returns `(body, is_structured)`.
fn build_request(model: &ModelDetails, query: ChatQuery) -> Result<(Value, bool), Error> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
    } = query;

    let structured_schema = thread
        .events
        .last()
        .and_then(|e| e.event.as_chat_request())
        .and_then(|req| req.schema.clone());

    let is_structured = structured_schema.is_some();
    let slug = model.id.name.to_string();

    let messages = thread.into_messages(to_system_messages, convert_events)?;
    let converted_tools = convert_tools(tools, &tool_choice);
    let tool_choice_val = convert_tool_choice(&tool_choice);

    trace!(
        slug,
        messages_size = messages.len(),
        tools_size = converted_tools.len(),
        "Built Llamacpp request."
    );

    let mut body = json!({
        "model": slug,
        "messages": messages,
        "stream": true,
    });

    if !converted_tools.is_empty() {
        body["tools"] = json!(converted_tools);
        body["tool_choice"] = json!(tool_choice_val);
    }

    if let Some(schema) = structured_schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": "structured_output",
                "schema": schema,
                "strict": true,
            },
        });
    }

    Ok((body, is_structured))
}

/// Convert system prompt parts into a list of JSON message values.
fn to_system_messages(parts: Vec<String>) -> impl Iterator<Item = Value> {
    parts
        .into_iter()
        .map(|content| json!({ "role": "system", "content": content }))
}

/// Convert a conversation event stream into a list of JSON message values.
fn convert_events(events: ConversationStream) -> Vec<Value> {
    events
        .into_iter()
        .filter_map(|event| match event.into_kind() {
            EventKind::ChatRequest(request) => {
                Some(json!({ "role": "user", "content": request.content }))
            }
            EventKind::ChatResponse(response) => match response {
                ChatResponse::Message { message } => {
                    Some(json!({ "role": "assistant", "content": message }))
                }
                ChatResponse::Reasoning { reasoning } => {
                    // Wrap reasoning in <think> tags so the model can pick up
                    // its own chain-of-thought on the next turn.
                    Some(json!({
                        "role": "assistant",
                        "content": format!("<think>\n{reasoning}\n</think>"),
                    }))
                }
                ChatResponse::Structured { data } => {
                    Some(json!({ "role": "assistant", "content": data.to_string() }))
                }
            },
            EventKind::ToolCallRequest(request) => Some(json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": request.id,
                    "type": "function",
                    "function": {
                        "name": request.name,
                        "arguments": Value::Object(request.arguments).to_string(),
                    },
                }],
            })),
            EventKind::ToolCallResponse(ToolCallResponse { id, result }) => Some(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": match result {
                    Ok(content) | Err(content) => content,
                },
            })),
            _ => None,
        })
        .fold(vec![], |mut messages: Vec<Value>, message| {
            // Merge consecutive assistant messages that carry tool_calls
            // (same folding logic as the old openai-crate implementation).
            if message.get("tool_calls").is_some()
                && let Some(last) = messages.last_mut()
                && last.get("tool_calls").is_some()
                && let (Some(existing), Some(new)) = (
                    last["tool_calls"].as_array_mut(),
                    message["tool_calls"].as_array(),
                )
            {
                existing.extend(new.iter().cloned());
                return messages;
            }
            messages.push(message);
            messages
        })
}

/// Convert tool definitions to the OpenAI-compatible JSON format.
///
/// If [`ToolChoice::Function`] is set, only include the named tool. llama.cpp
/// doesn't support calling a specific tool by name, but it supports `required`
/// mode, so we limit the tool list instead.
fn convert_tools(tools: Vec<ToolDefinition>, tool_choice: &ToolChoice) -> Vec<Value> {
    tools
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description.unwrap_or_default(),
                    "parameters": parameters_with_strict_mode(tool.parameters, true),
                    "strict": true,
                },
            })
        })
        .filter(|tool| match tool_choice {
            ToolChoice::Function(req) => tool["function"]["name"].as_str() == Some(req.as_str()),
            _ => true,
        })
        .collect()
}

fn convert_tool_choice(choice: &ToolChoice) -> &str {
    match choice {
        ToolChoice::Auto => "auto",
        ToolChoice::None => "none",
        ToolChoice::Required | ToolChoice::Function(_) => "required",
    }
}

fn map_model(model: &ModelResponse) -> Result<ModelDetails, Error> {
    Ok(ModelDetails {
        id: (
            PROVIDER,
            model
                .id
                .rsplit_once('/')
                .map_or(model.id.as_str(), |(_, v)| v),
        )
            .try_into()?,
        display_name: None,
        context_window: None,
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        features: vec![],
    })
}

impl TryFrom<&LlamacppConfig> for Llamacpp {
    type Error = Error;

    fn try_from(config: &LlamacppConfig) -> Result<Self, Self::Error> {
        let reqwest_client = reqwest::Client::builder().build()?;
        let base_url = config.base_url.clone();

        Ok(Llamacpp {
            reqwest_client,
            base_url,
        })
    }
}

// These mirror the llama.cpp server's `common_chat_msg_diff_to_json_oaicompat`
// output. The critical addition over the `openai` crate is the
// `reasoning_content` field, which carries extracted reasoning for the
// `--reasoning-format deepseek` (default) and `deepseek-legacy` modes.

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    /// Reasoning content extracted by the server (deepseek / deepseek-legacy).
    /// This is a non-standard `DeepSeek` extension that llama.cpp also uses.
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[cfg(test)]
#[path = "llamacpp_tests.rs"]
mod tests;
