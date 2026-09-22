//! The OpenAI chat-completions wire format, shared by the providers that speak
//! it.
//!
//! This is the `/v1/chat/completions` dialect: SSE chunks carrying
//! `choices[].delta`, terminated by a `[DONE]` sentinel.
//! The `cerebras`, `llamacpp`, and `vllm` providers all stream it.
//! It is a different protocol from the OpenAI Responses API that the `openai`
//! provider uses, which has its own typed event enum.
//!
//! The chunk types mirror llama.cpp's `common_chat_msg_diff_to_json_oaicompat`
//! output.
//! Their addition over the plain OpenAI shape is `reasoning_content`, which
//! carries extracted reasoning for llama.cpp's `--reasoning-format deepseek`
//! (default) and `deepseek-legacy` modes; Cerebras spells the same field
//! `reasoning`, and a serde alias accepts both.
//!
//! Deserialization is deliberately lenient: unknown fields are ignored and the
//! optional fields default, so a provider adding a field to a chunk does not
//! break the stream.
//! `StreamChoice::delta` is the one required field; a chunk whose choice omits
//! it fails to parse, and the provider logs a warning and skips that chunk.

use std::mem;

use futures::{Stream, StreamExt as _, future, stream};
use jp_config::assistant::tool_choice::ToolChoice;
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, EventKind, ToolCallResponse},
};
use reqwest_eventsource::Event as SseEvent;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, trace, warn};

use super::{EventStream, openai::parameters_with_strict_mode};
use crate::{
    error::StreamError,
    event::{Event, FinishReason},
    stream::aggregator::reasoning::ReasoningExtractor,
    tool::ToolDefinition,
};

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,

    /// An error the provider reported inside the stream, once the response had
    /// already returned 200.
    ///
    /// Held as raw JSON because nothing interprets the provider's error schema;
    /// [`parse_chunk`] hands the payload back so the caller can fail the stream
    /// rather than complete it on partial output. vLLM sends one when
    /// generation fails mid-stream and then closes with the `[DONE]` sentinel,
    /// which would otherwise read as an ordinary finish.
    #[serde(default)]
    pub error: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StreamChoice {
    pub delta: StreamDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

impl StreamChoice {
    /// Whether this choice carries any field the providers turn into events.
    fn is_actionable(&self) -> bool {
        self.finish_reason.is_some()
            || self.delta.content.is_some()
            || self.delta.reasoning_content.is_some()
            || self.delta.tool_calls.is_some()
    }
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

/// Parse one SSE `data:` payload from `provider` into a chunk.
///
/// Returns `Ok(None)` when the payload yields nothing to emit, having logged
/// the reason.
/// Returns `Err` with the provider's raw error payload when the payload reports
/// a failure rather than carrying generated output.
pub(crate) fn parse_chunk(data: &str, provider: &str) -> Result<Option<StreamChunk>, Value> {
    let mut chunk: StreamChunk = match serde_json::from_str(data) {
        Ok(chunk) => chunk,
        Err(error) => {
            warn!(provider, %error, data, "Failed to parse chunk.");
            return Ok(None);
        }
    };

    if let Some(error) = chunk.error.take() {
        warn!(
            provider,
            error = %error,
            "Provider reported an error inside the stream."
        );
        return Err(error);
    }

    if chunk.choices.is_empty() {
        debug!(provider, data, "Chunk carried no choices.");
        return Ok(None);
    }

    if !chunk.choices.iter().any(StreamChoice::is_actionable) {
        debug!(provider, data, "Chunk carried no actionable choices.");
        return Ok(None);
    }

    Ok(Some(chunk))
}

/// The human-readable message from an in-stream error payload.
///
/// Reads the payload's `message`, and falls back to the whole value when it
/// carries none, so nothing the server said is dropped on the way to the user.
fn stream_error_message(payload: &Value) -> String {
    payload
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| payload.to_string(), str::to_owned)
}

/// Merge consecutive assistant messages in an OpenAI-compatible
/// chat-completions message list into single messages.
///
/// A single model turn can surface reasoning, content, and several parallel
/// tool calls as separate events.
/// The chat-completions contract expects them as one assistant message:
/// reasoning and content folded in, every parallel `tool_calls` entry in one
/// array, immediately followed by the tool results.
///
/// Both the `reasoning` (Cerebras) and `reasoning_content` (llama.cpp) field
/// names are handled.
pub(crate) fn merge_consecutive_assistant_messages(messages: Vec<Value>) -> Vec<Value> {
    messages
        .into_iter()
        .fold(vec![], |mut acc: Vec<Value>, message| {
            if let Some(last) = acc.last_mut()
                && last.get("role").and_then(Value::as_str) == Some("assistant")
                && message.get("role").and_then(Value::as_str) == Some("assistant")
            {
                if let Some(new) = message.get("tool_calls").and_then(Value::as_array) {
                    last["tool_calls"]
                        .as_array_mut()
                        .map(|existing| existing.extend(new.iter().cloned()))
                        .unwrap_or_else(|| last["tool_calls"] = json!(new));
                }

                for key in ["reasoning", "reasoning_content", "content"] {
                    if let Some(value) = message.get(key)
                        && value.is_string()
                    {
                        last[key] = value.clone();
                    }
                }

                return acc;
            }

            acc.push(message);
            acc
        })
}

/// Convert system prompt parts into a list of JSON message values.
pub(crate) fn to_system_messages(parts: Vec<String>) -> impl Iterator<Item = Value> {
    parts
        .into_iter()
        .map(|content| json!({ "role": "system", "content": content }))
}

/// Convert a conversation event stream into a list of JSON message values.
pub(crate) fn convert_events(events: ConversationStream) -> Vec<Value> {
    let messages = events
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
        .collect();

    merge_consecutive_assistant_messages(messages)
}

/// Convert tool definitions to the OpenAI-compatible JSON format.
///
/// If [`ToolChoice::Function`] is set, only include the named tool. llama.cpp
/// doesn't support naming a tool in `tool_choice`, so the list is narrowed to
/// one tool and paired with `required` mode instead, which reaches the same
/// outcome on every server speaking this dialect.
pub(crate) fn convert_tools(tools: Vec<ToolDefinition>, tool_choice: &ToolChoice) -> Vec<Value> {
    tools
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.docs.schema_description().unwrap_or_default(),
                    "parameters": parameters_with_strict_mode(&tool.parameters, true),
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

pub(crate) fn convert_tool_choice(choice: &ToolChoice) -> &'static str {
    match choice {
        ToolChoice::Auto => "auto",
        ToolChoice::None => "none",
        ToolChoice::Required | ToolChoice::Function(_) => "required",
    }
}

/// Assemble the provider-agnostic event stream from a raw SSE event source.
///
/// `provider` names the server in log lines.
pub(crate) fn assemble_event_stream<S>(
    events: S,
    provider: &'static str,
    is_structured: bool,
) -> EventStream
where
    S: Stream<Item = std::result::Result<SseEvent, reqwest_eventsource::Error>> + Send + 'static,
{
    let mut state = StreamState::new(provider, is_structured);

    let mut seen_error = false;
    events
        .take_while(move |event| {
            // Include the first error before stopping: it must reach the
            // handler below to be surfaced (or dropped once finished), and
            // stopping prevents the EventSource from reconnecting after a
            // terminal error.
            let keep = !seen_error;
            if event.is_err() {
                seen_error = true;
            }
            future::ready(keep)
        })
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
        .boxed()
}

/// Mutable state carried across SSE events in a single stream.
pub(crate) struct StreamState {
    /// The server name for log lines.
    provider: &'static str,
    extractor: ReasoningExtractor,
    /// Tracks which tool call indices have been seen, so we can flush them on
    /// finish.
    pub(crate) tool_call_indices: Vec<usize>,
    reasoning_flushed: bool,
    /// Tracks whether `Event::flush(1)` (the message/structured index) has
    /// already been emitted in this stream.
    /// Without this gate, the `finish_reason` chunk and the `[DONE]` sentinel
    /// both emit it, producing a spurious second flush that downstream
    /// consumers can misinterpret as a re-dispatch signal.
    message_flushed: bool,
    /// Whether the terminal `Finished` event has been emitted.
    /// Once set, a subsequent stream error is the benign connection close that
    /// follows `[DONE]` and is dropped rather than surfaced to the retry layer.
    finished: bool,
    /// Whether the provider reported a failure inside the stream.
    /// The failure has already been surfaced as a [`StreamError`], so the
    /// `[DONE]` that follows must not flush the partial output or report a
    /// successful finish on top of it.
    errored: bool,
    /// Captured from `finish_reason` in the last choice delta.
    /// Emitted as `Event::Finished` when the `[DONE]` sentinel arrives.
    pub(crate) finish_reason: Option<FinishReason>,
    /// Set when server-separated reasoning arrives, and cleared by the first
    /// content frame that holds anything other than newlines.
    /// While set, leading newlines are stripped from each content frame.
    trim_content_prefix: bool,
    /// Whether the server has separated reasoning from content in this stream.
    ///
    /// The `reasoning_content` field rides the reasoning frames only; the
    /// answer frames that follow carry `content` alone.
    /// Remembering that the server does its own separation keeps the answer
    /// away from the `<think>` extractor, which would otherwise treat a literal
    /// tag in the answer as a reasoning block and move the text behind it out
    /// of the message.
    server_separated_reasoning: bool,
    is_structured: bool,
}

impl StreamState {
    pub(crate) fn new(provider: &'static str, is_structured: bool) -> Self {
        Self {
            provider,
            extractor: ReasoningExtractor::default(),
            tool_call_indices: Vec::new(),
            reasoning_flushed: false,
            message_flushed: false,
            finished: false,
            errored: false,
            finish_reason: None,
            trim_content_prefix: false,
            server_separated_reasoning: false,
            is_structured,
        }
    }
}

type SseResult = std::result::Result<Vec<Result<Event, StreamError>>, reqwest_eventsource::Error>;

/// Process a single SSE event into zero or more provider-agnostic events.
#[expect(clippy::too_many_lines)]
pub(crate) fn handle_sse_event_sync(
    event: Result<SseEvent, reqwest_eventsource::Error>,
    state: &mut StreamState,
) -> SseResult {
    match event {
        Ok(SseEvent::Open) => Ok(vec![]),
        Ok(SseEvent::Message(msg)) => {
            trace!(provider = state.provider, event = %msg.data, "Received event.");

            if msg.data == "[DONE]" {
                // The provider already reported a failure and it has been
                // surfaced: what arrived before it is a truncated answer, not a
                // completed one. Marking the stream finished keeps the
                // connection close that follows from surfacing a second error.
                if state.errored {
                    state.finished = true;
                    return Ok(vec![]);
                }

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

                // Flush message content if we never did.
                if !state.message_flushed {
                    events.push(Ok(Event::flush(1)));
                    state.message_flushed = true;
                }

                // Drain any tool call indices that weren't flushed via
                // `finish_reason`. In well-behaved streams this is empty —
                // the safety net guards against a missing `finish_reason`
                // chunk that would otherwise orphan the tool call buffer.
                for index in state.tool_call_indices.drain(..) {
                    events.push(Ok(Event::flush(index)));
                }

                events.push(Ok(Event::Finished(
                    state
                        .finish_reason
                        .take()
                        .unwrap_or(FinishReason::Completed),
                )));
                state.finished = true;
                return Ok(events);
            }

            let chunk = match parse_chunk(&msg.data, state.provider) {
                Ok(Some(chunk)) => chunk,
                Ok(None) => return Ok(vec![]),
                // Generation failed after the response had already returned
                // 200. Surface it as transient: the request itself was
                // accepted, so a fresh attempt is worth making, and the retry
                // layer reports the server's own message if they all fail.
                Err(payload) => {
                    state.errored = true;
                    return Ok(vec![Err(StreamError::transient(stream_error_message(
                        &payload,
                    )))]);
                }
            };

            let mut events = Vec::new();

            for choice in &chunk.choices {
                let delta = &choice.delta;

                // Reasoning via `reasoning_content` (deepseek / deepseek-legacy formats)
                //
                // The field's presence is what marks the server as one that
                // separates reasoning itself, so it is recorded even when this
                // frame carries no reasoning text to emit.
                if delta.reasoning_content.is_some() {
                    state.server_separated_reasoning = true;
                }

                if let Some(reasoning) = &delta.reasoning_content
                    && !reasoning.is_empty()
                {
                    events.push(Ok(Event::reasoning(0, reasoning.clone())));
                    state.trim_content_prefix = true;
                }

                // Content
                //
                // Once the server has sent reasoning of its own, content is
                // pure answer text. Otherwise content may carry <think> tags
                // (none format) and needs the extractor.
                if let Some(content) = &delta.content
                    && !content.is_empty()
                {
                    // A chat template separates the reasoning block from the
                    // answer with newlines. Some servers (vLLM) send them as
                    // content, others (llama.cpp) strip them first; dropping
                    // them here gives every provider the same answer text.
                    //
                    // Newlines only. Leading spaces belong to the answer, such
                    // as the indentation on a line of code.
                    let content = if state.trim_content_prefix {
                        content.trim_start_matches('\n')
                    } else {
                        content.as_str()
                    };

                    if !content.is_empty() {
                        state.trim_content_prefix = false;

                        if state.server_separated_reasoning {
                            flush_reasoning_if_needed(&mut events, &mut state.reasoning_flushed);

                            if state.is_structured {
                                events.push(Ok(Event::structured(1, content.to_owned())));
                            } else {
                                events.push(Ok(Event::message(1, content.to_owned())));
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
                }

                // Tool calls
                if delta.tool_calls.is_some() {
                    // A tool call terminates this message's content. Release
                    // any extractor-held tail now, before the tool-call parts
                    // are emitted: downstream drains the in-progress markdown
                    // paragraph at the tool-call boundary, so a tail released
                    // afterwards would land in a fresh paragraph and render as
                    // a mid-word blank-line split.
                    state.extractor.finalize();
                    events.extend(
                        drain_extractor(&mut state.extractor, state.is_structured)
                            .into_iter()
                            .map(Ok),
                    );
                }

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
                    if !state.message_flushed {
                        events.push(Ok(Event::flush(1)));
                        state.message_flushed = true;
                    }

                    if matches!(reason.as_str(), "tool_calls" | "stop") {
                        for index in state.tool_call_indices.drain(..) {
                            events.push(Ok(Event::flush(index)));
                        }
                    }

                    // Per the OpenAI spec.
                    match reason.as_str() {
                        "length" => {
                            // Active tool-call blocks are structurally
                            // incomplete when the model hits the token
                            // limit. Drop them here so the `[DONE]` safety
                            // net does not commit truncated arguments —
                            // mirrors `EventBuilder::drain` and the Google
                            // provider's MaxTokens behaviour.
                            state.tool_call_indices.clear();
                            state.finish_reason = Some(FinishReason::MaxTokens);
                        }
                        "stop" => state.finish_reason = Some(FinishReason::Completed),
                        _ => {}
                    }
                }
            }

            Ok(events)
        }
        Err(e) => {
            // A stream error after `Finished` is the benign close that
            // follows `[DONE]`; drop it. Before completion it's a real
            // transport failure (a dropped or stalled connection) that must
            // surface so the retry layer can act on it.
            if state.finished { Ok(vec![]) } else { Err(e) }
        }
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

#[cfg(test)]
#[path = "openai_compat_tests.rs"]
mod tests;
