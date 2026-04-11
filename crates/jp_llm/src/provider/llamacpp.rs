use std::mem;

use async_trait::async_trait;
use base64::Engine as _;
use futures::{StreamExt as _, future, stream};
use jp_attachment::AttachmentContent;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ReasoningConfig,
    },
    providers::llm::llamacpp::LlamacppConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, EventKind, ToolCallResponse},
    thread::text_attachments_to_xml,
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
    stream::aggregator::reasoning::ReasoningExtractor,
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
            tool_call_indices: Vec::new(),
            reasoning_flushed: false,
            finish_reason: None,
            is_structured,
        };

        Ok(es
            // EventSource yields Err on close; stop the stream.
            .take_while(|event| future::ready(event.is_ok()))
            .then(move |event| {
                let result = handle_sse_event_sync(event, &mut state);
                async move {
                    match result {
                        Ok(v) => stream::iter(v).boxed(),
                        Err(e) => {
                            stream::iter(vec![Err(StreamError::from_eventsource(e).await)]).boxed()
                        }
                    }
                }
            })
            .flatten()
            .boxed())
    }
}

/// Mutable state carried across SSE events in a single stream.
struct StreamState {
    extractor: ReasoningExtractor,
    /// Tracks which tool call indices have been seen, so we can flush them
    /// on finish.
    tool_call_indices: Vec<usize>,
    reasoning_flushed: bool,
    /// Captured from `finish_reason` in the last choice delta. Emitted as
    /// `Event::Finished` when the `[DONE]` sentinel arrives.
    finish_reason: Option<FinishReason>,
    is_structured: bool,
}

type SseResult = std::result::Result<Vec<Result<Event, StreamError>>, reqwest_eventsource::Error>;

/// Process a single SSE event into zero or more provider-agnostic events.
#[expect(clippy::too_many_lines)]
fn handle_sse_event_sync(
    event: Result<SseEvent, reqwest_eventsource::Error>,
    state: &mut StreamState,
) -> SseResult {
    match event {
        Ok(SseEvent::Open) => Ok(vec![]),
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
                return Ok(events);
            }

            let chunk: StreamChunk = match serde_json::from_str(&msg.data) {
                Ok(c) => c,
                Err(error) => {
                    warn!(
                        error = error.to_string(),
                        data = &msg.data,
                        "Failed to parse Llamacpp chunk."
                    );

                    return Ok(vec![]);
                }
            };

            let mut events = Vec::new();

            for choice in &chunk.choices {
                let delta = &choice.delta;

                // Reasoning via `reasoning_content` (deepseek / deepseek-legacy formats)
                if let Some(reasoning) = &delta.reasoning_content
                    && !reasoning.is_empty()
                {
                    events.push(Ok(Event::reasoning(0, reasoning.clone())));
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

                        if state.is_structured {
                            events.push(Ok(Event::structured(1, content.clone())));
                        } else {
                            events.push(Ok(Event::message(1, content.clone())));
                        }
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

                        if !state.tool_call_indices.contains(&index) {
                            state.tool_call_indices.push(index);
                        }

                        let id = tc.id.clone().unwrap_or_default();
                        let name = tc
                            .function
                            .as_ref()
                            .and_then(|f| f.name.clone())
                            .unwrap_or_default();
                        if !id.is_empty() || !name.is_empty() {
                            events.push(Ok(Event::tool_call_start(index, id, name)));
                        }

                        if let Some(args) =
                            tc.function.as_ref().and_then(|f| f.arguments.as_deref())
                        {
                            events.push(Ok(Event::tool_call_args(index, args)));
                        }
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

                    // Flush reasoning and message content before tool calls
                    // so they appear earlier in the conversation history.
                    if !state.reasoning_flushed {
                        events.push(Ok(Event::flush(0)));
                        state.reasoning_flushed = true;
                    }
                    events.push(Ok(Event::flush(1)));

                    if matches!(reason.as_str(), "tool_calls" | "stop") {
                        for &index in &state.tool_call_indices {
                            events.push(Ok(Event::flush(index)));
                        }
                        state.tool_call_indices.clear();
                    }

                    // Per the OpenAI spec.
                    match reason.as_str() {
                        "length" => state.finish_reason = Some(FinishReason::MaxTokens),
                        "stop" => state.finish_reason = Some(FinishReason::Completed),
                        _ => {}
                    }
                }
            }

            Ok(events)
        }
        Err(e) => Err(e),
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
        events.push(Event::reasoning(0, reasoning));
    }

    if !extractor.other.is_empty() {
        let content = mem::take(&mut extractor.other);
        if is_structured {
            events.push(Event::structured(1, content));
        } else {
            events.push(Event::message(1, content));
        }
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

    let structured_schema = thread.events.schema();

    let is_structured = structured_schema.is_some();
    let config = thread.events.config()?;
    let parameters = &config.assistant.model.parameters;
    let slug = model.id.name.to_string();

    let parts = thread.into_parts();

    let mut system_parts = parts.system_parts;
    if let Some(xml) = text_attachments_to_xml(&parts.attachments)? {
        system_parts.push(xml);
    }

    let mut messages: Vec<Value> = to_system_messages(system_parts).collect();

    // Prepend binary image attachments as a user message with image_url
    // content blocks (OpenAI chat completions format).
    let image_blocks: Vec<_> = parts
        .attachments
        .iter()
        .filter_map(|a| match &a.content {
            AttachmentContent::Binary { data, media_type } if media_type.starts_with("image/") => {
                Some(json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!(
                            "data:{media_type};base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(data),
                        ),
                    },
                }))
            }
            AttachmentContent::Binary { media_type, .. } => {
                warn!(
                    source = %a.source,
                    media_type,
                    "Unsupported binary attachment media type for llama.cpp, skipping."
                );
                None
            }
            AttachmentContent::Text(_) => None,
        })
        .collect();

    if !image_blocks.is_empty() {
        messages.push(json!({
            "role": "user",
            "content": image_blocks,
        }));
    }

    messages.extend(convert_events(parts.events));
    let converted_tools = convert_tools(tools, &tool_choice);
    let tool_choice_val = convert_tool_choice(&tool_choice);

    trace!(
        slug,
        messages_size = messages.len(),
        tools_size = converted_tools.len(),
        "Built Llamacpp request."
    );

    // Like Ollama, llama.cpp models may default to thinking-on, so we
    // explicitly control reasoning via `reasoning_format` and
    // `chat_template_kwargs.enable_thinking`. The former tells the server how
    // to surface reasoning tokens (separate field vs raw in content); the
    // latter tells the chat template whether to prompt the model to think at
    // all. Models whose template doesn't use `enable_thinking` silently
    // ignore the extra kwarg.
    let reasoning_enabled = !matches!(parameters.reasoning, None | Some(ReasoningConfig::Off));

    let mut body = json!({
        "model": slug,
        "messages": messages,
        "stream": true,
        "reasoning_format": if reasoning_enabled { "deepseek" } else { "none" },
        "chat_template_kwargs": { "enable_thinking": reasoning_enabled },
    });

    if let Some(temperature) = parameters.temperature {
        body["temperature"] = json!(temperature);
    }

    if let Some(top_p) = parameters.top_p {
        body["top_p"] = json!(top_p);
    }

    if let Some(max_tokens) = parameters.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }

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
                    // Use the `reasoning_content` field so the server can
                    // apply the correct template formatting. This avoids
                    // manually wrapping in `<think>` tags.
                    Some(json!({
                        "role": "assistant",
                        "reasoning_content": reasoning,
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
            if let Some(last) = messages.last_mut()
                && last.get("role").and_then(Value::as_str) == Some("assistant")
                && message.get("role").and_then(Value::as_str) == Some("assistant")
            {
                // Merge consecutive assistant messages: fold
                // `reasoning_content`, `content`, and `tool_calls` into a
                // single message.
                if let Some(tool_calls) = message.get("tool_calls")
                    && let Some(new) = tool_calls.as_array()
                {
                    last["tool_calls"]
                        .as_array_mut()
                        .map(|existing| existing.extend(new.iter().cloned()))
                        .unwrap_or_else(|| last["tool_calls"] = json!(new));
                }

                if let Some(rc) = message.get("reasoning_content")
                    && rc.is_string()
                {
                    last["reasoning_content"] = rc.clone();
                }

                if let Some(c) = message.get("content")
                    && c.is_string()
                {
                    last["content"] = c.clone();
                }

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
                    "description": tool.docs.schema_description().unwrap_or_default(),
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
        structured_output: None,
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
//
// These types are also used by the `cerebras` provider, which streams an
// identical OpenAI-compatible SSE format.

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChoice {
    pub delta: StreamDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct StreamDelta {
    #[serde(default)]
    pub content: Option<String>,
    /// Reasoning content extracted by the server (deepseek / deepseek-legacy).
    /// This is a non-standard `DeepSeek` extension that llama.cpp also uses.
    #[serde(default, alias = "reasoning")]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolCallDelta {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[cfg(test)]
#[path = "llamacpp_tests.rs"]
mod tests;
