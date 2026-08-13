use std::{env, time::Duration};

use async_stream::try_stream;
use async_trait::async_trait;
use base64::Engine as _;
use chrono::NaiveDate;
use futures::{StreamExt as _, TryStreamExt as _, pin_mut, stream};
use jp_attachment::AttachmentContent;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ReasoningEffort,
    },
    providers::llm::openrouter::OpenrouterConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, ConversationEvent, EventKind},
    thread::{Thread, ThreadParts, text_attachments_to_xml},
};
use jp_openrouter::{
    Client,
    types::{
        self,
        chat::{CacheControl, Content, FilePayload, Message},
        request::{self, JsonSchemaFormat, RequestMessage, ResponseFormat},
        response::{
            self, ChatCompletion as OpenRouterChunk, FinishReason, ReasoningDetails,
            ReasoningDetailsFormat, ReasoningDetailsKind,
        },
        tool::{self, FunctionCall, Tool, ToolCall, ToolCallType, ToolFunction},
    },
};
use serde::Serialize;
use serde_json::{Map, Value};
use tracing::{debug, error, info, trace, warn};

use super::{EventStream, ModelDetails};
use crate::{
    Error, StreamError,
    error::{Result, StreamErrorKind, looks_like_quota_error},
    event::{self, Event, EventPart, ToolCallPart},
    event_builder::EventBuilder,
    model::ReasoningDetails as ModelReasoningDetails,
    provider::{Provider, openai::parameters_with_strict_mode, trace_to_tmpfile},
    query::ChatQuery,
    stream::with_tool_call_keepalive,
};

static PROVIDER: ProviderId = ProviderId::Openrouter;

const ANTHROPIC_REDACTED_THINKING_KEY: &str = "anthropic_redacted_thinking";
const ANTHROPIC_THINKING_SIGNATURE_KEY: &str = "anthropic_thinking_signature";
const GOOGLE_THOUGHT_SIGNATURE_KEY: &str = "google_thought_signature";
const OPENAI_ENCRYPTED_CONTENT_KEY: &str = "openai_encrypted_content";

/// How often to inject a synthetic keep-alive while a tool call is streaming.
///
/// OpenRouter proxies to upstream providers (Anthropic, OpenAI, ...) that emit
/// tool-call arguments as a burst after a silent gap, which the idle timeout
/// would otherwise treat as a dead connection.
/// Stays below the enforced minimum `stream_idle_timeout_secs` (10s) so the
/// heartbeat always lands before the idle window elapses.
const TOOL_CALL_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
const SOFT_FORCE_MAX_RETRIES: u8 = 3;

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

    /// Set the base URL for the Openrouter API.
    fn with_base_url(mut self, base_url: String) -> Self {
        self.client = self.client.with_base_url(base_url);
        self
    }
}

/// How to retry when a reasoning request cannot carry a forced tool choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForceStrategy {
    /// Disable reasoning and restore the requested tool choice for one retry.
    DisableThinking,

    /// Keep reasoning active and retry with progressively stronger nudges.
    EscalatingNudge { remaining: u8 },
}

/// The original forced tool choice and the retry strategy selected for it.
#[derive(Debug, Clone)]
struct ForcedToolFallback {
    tool_choice: tool::ToolChoice,
    strategy: ForceStrategy,
}

impl ForcedToolFallback {
    /// Whether the response called the tool requested by the original choice.
    fn is_satisfied_by(&self, tool_names_called: &[String]) -> bool {
        match &self.tool_choice {
            tool::ToolChoice::Function(function) => tool_names_called
                .iter()
                .any(|name| name == &function.function.name),
            tool::ToolChoice::Required => !tool_names_called.is_empty(),
            tool::ToolChoice::Auto | tool::ToolChoice::None => true,
        }
    }
}

#[async_trait]
impl Provider for Openrouter {
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
        let mut models = map_models(self.client.models().await?.data);

        models.sort_by(|a, b| a.id.cmp(&b.id));
        models.dedup();

        Ok(models)
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream> {
        let (request, is_structured, forced_tool_fallback) = create_request(model, query)?;

        Ok(call(
            self.client.clone(),
            request,
            is_structured,
            forced_tool_fallback,
        ))
    }
}

fn call(
    client: Client,
    request: request::ChatCompletion,
    is_structured: bool,
    forced_tool_fallback: Option<ForcedToolFallback>,
) -> EventStream {
    Box::pin(try_stream! {
        debug!(stream = true, "OpenRouter chat completion stream request.");
        trace!(
            request = %trace_to_tmpfile("jp-openrouter-request", &request),
            "Request payload."
        );

        let mut state = AggregationState {
            tool_call_indices: Vec::new(),
            aggregating_reasoning: false,
            aggregating_message: false,
            is_structured,
        };
        let mut builder = EventBuilder::new();
        let mut events = vec![];
        let mut tool_names_called = vec![];

        let raw_stream = client
            .chat_completion_stream(request.clone())
            .map_err(StreamError::from)
            .map_ok(move |value| stream::iter(map_completion(value, &mut state)))
            .try_flatten()
            .boxed();
        let event_stream = with_tool_call_keepalive(raw_stream, TOOL_CALL_KEEPALIVE_INTERVAL);

        pin_mut!(event_stream);
        while let Some(result) = event_stream.next().await {
            match result? {
                Event::Finished(event::FinishReason::Completed)
                    if let Some(fallback) = forced_tool_fallback
                        .as_ref()
                        .filter(|fallback| !fallback.is_satisfied_by(&tool_names_called)) =>
                {
                    events.extend(builder.drain());
                    for await event in dispatch_force_retry(
                        client.clone(),
                        request.clone(),
                        events,
                        fallback,
                        is_structured,
                    ) {
                        yield event?;
                    }
                    return;
                }
                done @ Event::Finished(_) => {
                    yield done;
                    return;
                }
                Event::Part { index, part, metadata } => {
                    if let EventPart::ToolCall(ToolCallPart::Start { name, .. }) = &part {
                        tool_names_called.push(name.clone());
                    } else if forced_tool_fallback.is_some() && !part.is_tool_call() {
                        builder.handle_part(index, part.clone(), metadata.clone());
                    }

                    yield Event::Part { index, part, metadata };
                }
                flush @ Event::Flush { .. } => {
                    if forced_tool_fallback.is_some()
                        && let Event::Flush { index, metadata } = &flush
                    {
                        events.extend(builder.handle_flush(*index, metadata.clone()));
                    }
                    yield flush;
                }
                patch @ Event::Patch(_) => yield patch,
                keep_alive @ Event::KeepAlive => yield keep_alive,
            }
        }
    })
}

fn dispatch_force_retry(
    client: Client,
    request: request::ChatCompletion,
    events: Vec<ConversationEvent>,
    fallback: &ForcedToolFallback,
    is_structured: bool,
) -> EventStream {
    match fallback.strategy {
        ForceStrategy::DisableThinking => {
            info!(
                "Model did not call the required tool. Retrying with reasoning disabled and \
                 forced tool_choice."
            );
            force_tool_retry(client, request, events, fallback, is_structured)
        }
        ForceStrategy::EscalatingNudge { remaining } => {
            info!(
                remaining,
                "Model did not call the required tool. Retrying with a firmer nudge."
            );
            soft_force_retry(client, request, events, fallback, is_structured, remaining)
        }
    }
}

fn force_tool_retry(
    client: Client,
    request: request::ChatCompletion,
    events: Vec<ConversationEvent>,
    fallback: &ForcedToolFallback,
    is_structured: bool,
) -> EventStream {
    let request = prepare_force_tool_retry_request(request, events, fallback);
    call(client, request, is_structured, None)
}

fn prepare_force_tool_retry_request(
    mut request: request::ChatCompletion,
    events: Vec<ConversationEvent>,
    fallback: &ForcedToolFallback,
) -> request::ChatCompletion {
    request.messages.extend(convert_conversation_events(events));
    request.messages.push(
        Message::default()
            .with_text(force_retry_prompt(&fallback.tool_choice))
            .user(),
    );
    request.reasoning = Some(request::Reasoning {
        exclude: true,
        effort: request::ReasoningEffort::None,
    });
    request.tool_choice = Some(fallback.tool_choice.clone());
    request
}

fn soft_force_retry(
    client: Client,
    request: request::ChatCompletion,
    events: Vec<ConversationEvent>,
    fallback: &ForcedToolFallback,
    is_structured: bool,
    remaining: u8,
) -> EventStream {
    let (request, next_fallback) =
        prepare_soft_force_retry_request(request, events, fallback, remaining);
    call(client, request, is_structured, next_fallback)
}

fn prepare_soft_force_retry_request(
    mut request: request::ChatCompletion,
    events: Vec<ConversationEvent>,
    fallback: &ForcedToolFallback,
    remaining: u8,
) -> (request::ChatCompletion, Option<ForcedToolFallback>) {
    request.messages.extend(convert_conversation_events(events));

    let attempt = SOFT_FORCE_MAX_RETRIES - remaining + 1;
    let target = force_target(&fallback.tool_choice);
    let intensifier = match attempt {
        1 => "",
        2 => "You have now failed to do this twice. ",
        _ => "This is your final attempt. ",
    };
    request.messages.push(
        Message::default()
            .with_text(format!(
                "{intensifier}You did not call {target} as required. You MUST call {target} now. \
                 Do not produce any other text, reasoning, or questions; just make the tool call."
            ))
            .user(),
    );
    request.tool_choice = Some(tool::ToolChoice::Auto);

    let next_fallback = (remaining > 1).then(|| ForcedToolFallback {
        tool_choice: fallback.tool_choice.clone(),
        strategy: ForceStrategy::EscalatingNudge {
            remaining: remaining - 1,
        },
    });

    (request, next_fallback)
}

fn force_target(choice: &tool::ToolChoice) -> String {
    match choice {
        tool::ToolChoice::Function(function) => {
            format!("the tool named '{}'", function.function.name)
        }
        _ => "at least one tool".to_owned(),
    }
}

fn force_retry_prompt(choice: &tool::ToolChoice) -> String {
    match choice {
        tool::ToolChoice::Function(function) => format!(
            "You did not call the required tool. You MUST call the tool named '{}' now. Do not \
             respond with text.",
            function.function.name
        ),
        _ => "You did not call any tool. You MUST call at least one tool now. Do not respond with \
              text."
            .to_owned(),
    }
}

/// Aggregation state for a single stream of events.
struct AggregationState {
    /// Tracks which tool call indices have been seen, so we can flush them on
    /// finish.
    tool_call_indices: Vec<usize>,

    /// Did the stream of events have any reasoning content?
    aggregating_reasoning: bool,

    /// Did the stream of events have any message content?
    aggregating_message: bool,

    /// Whether the current request uses structured (JSON schema) output.
    is_structured: bool,
}

/// Metadata stored in the conversation stream, based on Openrouter
/// multi-provider support.
///
/// For example, if we use Openrouter to call an Openai model with reasoning
/// support, Openrouter will send us the "encryted reasoning" content in the
/// payload.
/// We take that data, and morph it into a certain metadata shape that can be
/// read by both the Openrouter and Openai provider implementations, such that
/// the reasoning content can be used in future turns, regardless of whether the
/// conversation keeps using the Openrouter provider, or switches to the Openai
/// provider.
/// The same applies to Anthropic, and other providers for which Openrouter has
/// provider-specific metadata support.
#[derive(Default, Serialize)]
struct MultiProviderMetadata {
    // Each field name below is the metadata key itself, so it has to match the
    // owning provider's constant exactly:
    // `crate::provider::openai::ENCRYPTED_CONTENT_KEY`,
    // `anthropic::THINKING_SIGNATURE_KEY`, `anthropic::REDACTED_THINKING_KEY`,
    // and `google::THOUGHT_SIGNATURE_KEY`.
    #[serde(skip_serializing_if = "Option::is_none")]
    openai_encrypted_content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anthropic_thinking_signature: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anthropic_redacted_thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    google_thought_signature: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    openrouter_metadata: Vec<Map<String, Value>>,
}

impl MultiProviderMetadata {
    fn from_details(details: Vec<ReasoningDetails>) -> Self {
        let mut metadata = Self::default();

        for details in details {
            let ReasoningDetails {
                id,
                format,
                index,
                kind,
            } = details;

            let field = match (format, kind) {
                (Some(format), ReasoningDetailsKind::Encrypted { data }) => match format {
                    ReasoningDetailsFormat::OpenaiResponsesV1 => {
                        metadata.openai_encrypted_content = Some(data.into());
                        OPENAI_ENCRYPTED_CONTENT_KEY
                    }
                    ReasoningDetailsFormat::AnthropicClaudeV1 => {
                        metadata.anthropic_redacted_thinking = Some(data.into());
                        ANTHROPIC_REDACTED_THINKING_KEY
                    }
                    _ => "",
                },
                (
                    Some(format),
                    ReasoningDetailsKind::Text {
                        signature: Some(signature),
                        ..
                    },
                ) => match format {
                    ReasoningDetailsFormat::AnthropicClaudeV1 => {
                        metadata.anthropic_thinking_signature = Some(signature.into());
                        ANTHROPIC_THINKING_SIGNATURE_KEY
                    }
                    ReasoningDetailsFormat::GoogleGeminiV1 => {
                        metadata.google_thought_signature = Some(signature.into());
                        GOOGLE_THOUGHT_SIGNATURE_KEY
                    }
                    _ => "",
                },
                _ => "",
            };

            let mut map = Map::new();
            if !field.is_empty() {
                if let Some(id) = id {
                    map.insert("id".into(), id.into());
                }

                if let Some(index) = index {
                    map.insert("index".into(), index.into());
                }

                map.insert("field".into(), field.into());
            }
            if !map.is_empty() {
                metadata.openrouter_metadata.push(map);
            }
        }

        metadata
    }
}

impl From<MultiProviderMetadata> for Map<String, Value> {
    fn from(val: MultiProviderMetadata) -> Self {
        // The field names are the metadata keys and the `skip_serializing_if`
        // attributes drop the absent ones, so serializing is the conversion.
        // Every field is already a `Value` or a `Vec` of them, which cannot
        // fail to serialize and cannot produce anything but an object.
        match serde_json::to_value(val) {
            Ok(Value::Object(map)) => map,
            other => {
                error!(
                    ?other,
                    "MultiProviderMetadata did not serialize to an object; dropping provider \
                     metadata for this event"
                );
                Map::new()
            }
        }
    }
}

fn map_completion(
    v: OpenRouterChunk,
    state: &mut AggregationState,
) -> Vec<std::result::Result<Event, StreamError>> {
    trace!(
        event = serde_json::to_string(&v).unwrap_or_default(),
        "Received event from OpenRouter API."
    );

    v.choices
        .into_iter()
        .flat_map(|v| map_event(v, state))
        .collect()
}

#[expect(clippy::too_many_lines)]
fn map_event(
    choice: types::response::Choice,
    state: &mut AggregationState,
) -> Vec<std::result::Result<Event, StreamError>> {
    let types::response::Choice::Streaming(types::response::StreamingChoice {
        finish_reason,
        delta:
            types::response::StreamingDelta {
                content,
                reasoning,
                tool_calls,
                reasoning_details,
                ..
            },
        error,
        ..
    }) = choice
    else {
        warn!("Received non-streaming choice in streaming context, ignoring.");
        return vec![];
    };

    // I _believe_ we can ignore the `reasoning.summary` details variant,
    // since it is basically a clone of the reasoning text we already have
    // in the regular `reasoning` field.
    let reasoning_details = reasoning_details
        .into_iter()
        .filter(|details| !matches!(details.kind, ReasoningDetailsKind::Summary { .. }))
        .collect::<Vec<_>>();

    let has_reasoning_details = !reasoning_details.is_empty();
    let reasoning_details = MultiProviderMetadata::from_details(reasoning_details);

    if let Some(error) = error {
        if looks_like_quota_error(&error.message) {
            return vec![Err(StreamError::new(
                StreamErrorKind::InsufficientQuota,
                format!(
                    "Insufficient API quota. Check your credits \
                     at https://openrouter.ai/settings/credits. ({})",
                    error.message
                ),
            ))];
        }
        return vec![Err(StreamError::other(error.message))];
    }

    let mut events = vec![];
    let reasoning = reasoning.unwrap_or_default();
    if !reasoning.is_empty() || has_reasoning_details {
        state.aggregating_reasoning = true;

        let metadata = if has_reasoning_details {
            reasoning_details.into()
        } else {
            Map::new()
        };

        events.push(Ok(Event::Part {
            index: 0,
            part: EventPart::Reasoning(reasoning),
            metadata,
        }));
    }

    if let Some(content) = content
        && !content.is_empty()
    {
        state.aggregating_message = true;

        if state.is_structured {
            events.push(Ok(Event::structured(1, content)));
        } else {
            events.push(Ok(Event::message(1, content)));
        }
    }

    if finish_reason.is_some() {
        if state.aggregating_reasoning {
            state.aggregating_reasoning = false;
            events.push(Ok(Event::flush(0)));
        }

        if state.aggregating_message {
            state.aggregating_message = false;
            events.push(Ok(Event::flush(1)));
        }
    }

    for (
        idx,
        types::tool::ToolCall {
            function,
            id,
            index,
            kind: _,
        },
    ) in tool_calls.into_iter().enumerate()
    {
        let index = idx + index + 2;

        if !state.tool_call_indices.contains(&index) {
            state.tool_call_indices.push(index);
        }

        let id = id.unwrap_or_default();
        let name = function.name.unwrap_or_default();
        if !id.is_empty() || !name.is_empty() {
            events.push(Ok(Event::tool_call_start(index, id, name)));
        }

        if let Some(args) = function.arguments.as_deref() {
            events.push(Ok(Event::tool_call_args(index, args)));
        }
    }

    if let Some(FinishReason::ToolCalls | FinishReason::Stop) = finish_reason {
        for &index in &state.tool_call_indices {
            events.push(Ok(Event::flush(index)));
        }
        state.tool_call_indices.clear();
    }

    match finish_reason {
        Some(FinishReason::Length) => {
            events.push(Ok(Event::Finished(event::FinishReason::MaxTokens)));
        }
        Some(FinishReason::Stop | FinishReason::ToolCalls) => {
            events.push(Ok(Event::Finished(event::FinishReason::Completed)));
        }
        Some(FinishReason::Error) => {
            events.push(Err(StreamError::other("unknown stream error")));
        }
        Some(reason) => events.push(Ok(Event::Finished(event::FinishReason::Other(
            reason.as_str().into(),
        )))),
        _ => {}
    }

    events
}

#[cfg(test)]
impl Openrouter {
    /// Build the OpenRouter wire request for `query` and serialize it to JSON
    /// without sending.
    /// Test-only seam for snapshotting request construction (notably compaction
    /// projection) across providers.
    #[expect(
        clippy::unused_self,
        reason = "uniform per-provider seam; only some providers read instance state"
    )]
    pub(crate) fn request_value(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<serde_json::Value> {
        let (request, _, _) = create_request(model, query)?;
        Ok(serde_json::to_value(request)?)
    }
}

fn convert_tool_choice(choice: ToolChoice) -> tool::ToolChoice {
    match choice {
        ToolChoice::Auto => tool::ToolChoice::Auto,
        ToolChoice::None => tool::ToolChoice::None,
        ToolChoice::Required => tool::ToolChoice::Required,
        ToolChoice::Function(name) => tool::ToolChoice::function(name),
    }
}

fn prepare_forced_tool_fallback(
    model: &ModelDetails,
    thinking_active: bool,
    tool_choice: &ToolChoice,
    messages: &mut RequestMessages,
) -> Option<ForcedToolFallback> {
    if !thinking_active || !tool_choice.is_forced_call() {
        return None;
    }

    info!(
        ?tool_choice,
        "OpenRouter routes forced tool choices to providers that may reject them while reasoning \
         is active. Switching to soft-force mode with fallback."
    );

    let strategy = if model.supports_disabling_thinking() {
        ForceStrategy::DisableThinking
    } else {
        ForceStrategy::EscalatingNudge {
            remaining: SOFT_FORCE_MAX_RETRIES,
        }
    };
    let fallback = ForcedToolFallback {
        tool_choice: convert_tool_choice(tool_choice.clone()),
        strategy,
    };

    messages.0.insert(
        0,
        Message::default()
            .with_text(force_nudge(&fallback.tool_choice))
            .system(),
    );

    Some(fallback)
}

fn force_nudge(choice: &tool::ToolChoice) -> String {
    match choice {
        tool::ToolChoice::Function(function) => format!(
            "IMPORTANT: You MUST call the tool named '{}'. DO NOT QUESTION THIS DIRECTIVE. DO NOT \
             PROMPT FOR MORE CONTEXT OR DETAILS. JUST RUN IT.",
            function.function.name
        ),
        _ => "IMPORTANT: You MUST use AT LEAST ONE tool available to you. DO NOT QUESTION THIS \
              DIRECTIVE. DO NOT PROMPT FOR MORE CONTEXT OR DETAILS. JUST RUN IT."
            .to_owned(),
    }
}

/// Create the request for the OpenRouter API.
///
/// Returns the request, whether structured output is active, and any fallback
/// required because reasoning prevents a forced tool choice on the wire.
fn create_request(
    model: &ModelDetails,
    query: ChatQuery,
) -> Result<(request::ChatCompletion, bool, Option<ForcedToolFallback>)> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
    } = query;

    let config = thread.events.config()?;
    let parameters = &config.assistant.model.parameters;

    let response_format = thread
        .events
        .schema()
        .map(|schema| ResponseFormat::JsonSchema {
            json_schema: JsonSchemaFormat {
                name: "structured_output".to_owned(),
                schema: Value::Object(schema),
                strict: Some(true),
            },
        });

    let is_structured = response_format.is_some();

    let slug = model.id.name.to_string();
    let reasoning = model.custom_reasoning_config(parameters.reasoning);

    let mut messages: RequestMessages = (&model.id, thread).try_into()?;
    let tools = tools
        .into_iter()
        .map(|tool| Tool::Function {
            function: ToolFunction {
                parameters: parameters_with_strict_mode(tool.parameters, true),
                name: tool.name,
                description: tool.docs.schema_description().map(str::to_owned),
                strict: true,
            },
        })
        .collect::<Vec<_>>();
    let thinking_active = reasoning.is_some()
        || model
            .reasoning
            .as_ref()
            .is_some_and(|details| !details.can_disable());
    let forced_tool_fallback =
        prepare_forced_tool_fallback(model, thinking_active, &tool_choice, &mut messages);

    let tool_choice = if tools.is_empty() {
        None
    } else if forced_tool_fallback.is_some() {
        Some(tool::ToolChoice::Auto)
    } else {
        Some(convert_tool_choice(tool_choice))
    };

    trace!(
        slug,
        messages_size = messages.0.len(),
        tools_size = tools.len(),
        "Built Openrouter request."
    );

    Ok((
        request::ChatCompletion {
            model: slug,
            messages: messages.0,
            // A reasoning object is always sent. Resolving the effort against
            // the model's ladder happens upstream in `custom_reasoning_config`,
            // so by this point the only question is how to express the result on
            // the wire.
            //
            // "Off" is expressed as `effort: minimal` + `exclude: true` rather
            // than `effort: none`, because some models (e.g. gpt-5-mini) reject
            // fully disabled reasoning; `minimal` is the floor every routed model
            // accepts. Reasoning tokens are always captured, and the display
            // layer decides what to show.
            reasoning: Some(match reasoning {
                Some(r) => request::Reasoning {
                    exclude: false,
                    effort: match r
                        .effort
                        .abs_to_rel(model.max_output_tokens)
                        .unwrap_or(ReasoningEffort::Auto)
                    {
                        ReasoningEffort::Max | ReasoningEffort::XHigh => {
                            request::ReasoningEffort::XHigh
                        }
                        ReasoningEffort::High => request::ReasoningEffort::High,
                        ReasoningEffort::Auto | ReasoningEffort::Medium => {
                            request::ReasoningEffort::Medium
                        }
                        ReasoningEffort::None | ReasoningEffort::Xlow => {
                            request::ReasoningEffort::Minimal
                        }
                        ReasoningEffort::Low => request::ReasoningEffort::Low,
                        ReasoningEffort::Absolute(_) => {
                            debug_assert!(false, "Reasoning effort must be relative.");
                            request::ReasoningEffort::Medium
                        }
                    },
                },
                None => request::Reasoning {
                    exclude: true,
                    effort: request::ReasoningEffort::Minimal,
                },
            }),
            tools,
            tool_choice,
            response_format,
            ..Default::default()
        },
        is_structured,
        forced_tool_fallback,
    ))
}

/// Map catalog entries to [`ModelDetails`], skipping entries that cannot be
/// parsed.
///
/// The catalog only enriches an already-validated model id with details, and
/// `model_details` tolerates a model missing from the catalog entirely.
/// An unparseable *unrelated* entry (e.g. Openrouter's `~`-prefixed rerouted
/// listings, which violate the model id character set) must therefore not fail
/// the whole fetch.
fn map_models(models: Vec<response::Model>) -> Vec<ModelDetails> {
    models
        .into_iter()
        .filter_map(|model| {
            let id = model.id.clone();
            map_model(model)
                .inspect_err(
                    |error| warn!(id, %error, "Skipping invalid model in Openrouter catalog."),
                )
                .ok()
        })
        .collect()
}

// TODO: Manually add a bunch of often-used models.
fn map_model(model: response::Model) -> Result<ModelDetails> {
    // An empty parameter list means the catalog reported nothing, which is not
    // the same as the model supporting nothing.
    let structured_output = (!model.supported_parameters.is_empty()).then(|| {
        model
            .supported_parameters
            .iter()
            .any(|p| p == "structured_outputs")
    });
    let reasoning = derive_reasoning(&model);

    Ok(ModelDetails {
        id: (PROVIDER, model.id).try_into()?,
        display_name: Some(model.name),
        // The serving provider's window, which may be smaller than the model's
        // own, is what a request actually gets.
        context_window: model
            .top_provider
            .context_length
            .or(Some(model.context_length)),
        max_output_tokens: model.top_provider.max_completion_tokens,
        reasoning,
        // Not `created`, which is when the model was listed on OpenRouter rather
        // than a training cutoff.
        knowledge_cutoff: model
            .knowledge_cutoff
            .as_deref()
            .and_then(parse_knowledge_cutoff),
        deprecated: None,
        structured_output,
        prefill: None,
        features: vec![],
    })
}

/// Parse a catalog knowledge cutoff, reported as `YYYY-MM-DD`.
///
/// An unparseable value is treated as absent rather than failing the listing:
/// the catalog is shared across many upstream providers and one malformed date
/// must not drop the whole model.
fn parse_knowledge_cutoff(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .inspect_err(|error| debug!(%raw, %error, "Ignoring unparseable knowledge cutoff."))
        .ok()
}

/// Derive reasoning support from a model's advertised capabilities.
///
/// Returns `None` when the model claims a reasoning parameter without
/// describing it, leaving support unknown rather than guessing a ladder.
fn derive_reasoning(model: &response::Model) -> Option<ModelReasoningDetails> {
    let Some(reasoning) = &model.reasoning else {
        // An empty parameter list means the catalog reported nothing, so support
        // stays unknown. Without this guard `all` holds vacuously and an
        // unreported model reads as "known: does not reason".
        if model.supported_parameters.is_empty() {
            return None;
        }

        // No reasoning block. A model that also advertises no reasoning
        // parameter does not reason; anything else is unknown.
        return model
            .supported_parameters
            .iter()
            .all(|p| p != "reasoning" && p != "reasoning_effort")
            .then(ModelReasoningDetails::unsupported);
    };

    // A reasoning block naming no efforts describes nothing, so support stays
    // unknown rather than becoming a known ladder with no rungs. Building one
    // would let `custom_reasoning_config` fall through to `xlow` and send a
    // `minimal` effort the catalog never announced.
    if reasoning.supported_efforts.is_empty() {
        return None;
    }

    let supports = |effort: &str| reasoning.supported_efforts.iter().any(|e| e == effort);

    let details = ModelReasoningDetails::leveled(
        supports("minimal") || supports("xlow"),
        supports("low"),
        supports("medium"),
        supports("high"),
        supports("xhigh"),
        supports("max"),
    );

    // `mandatory` is the one capability field any provider reports that maps
    // directly onto reasoning being impossible to turn off.
    Some(if reasoning.mandatory {
        details.always_on()
    } else {
        details
    })
}

impl From<types::response::ErrorResponse> for Error {
    fn from(error: types::response::ErrorResponse) -> Self {
        Self::OpenRouter(jp_openrouter::Error::Api {
            code: error.code,
            message: error.message,
        })
    }
}

impl TryFrom<&OpenrouterConfig> for Openrouter {
    type Error = Error;

    fn try_from(config: &OpenrouterConfig) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        let client = Openrouter::new(
            api_key,
            Some(config.app_name.clone()),
            config.app_referrer.clone(),
        )
        .with_base_url(config.base_url.clone());

        Ok(client)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct RequestMessages(pub Vec<RequestMessage>);

impl TryFrom<(&ModelIdConfig, Thread)> for RequestMessages {
    type Error = Error;

    fn try_from((model_id, thread): (&ModelIdConfig, Thread)) -> Result<Self> {
        let ThreadParts {
            system_parts,
            attachments,
            events,
        } = thread.into_parts();

        let mut messages = vec![];

        // Cache breakpoints on system parts:
        // - Always cache the first part (index 0)
        // - Cache the last part
        let mut system_content = vec![];
        let last_idx = system_parts.len().saturating_sub(1);

        for (i, text) in system_parts.into_iter().enumerate() {
            let cache = i == 0 || i == last_idx;
            system_content.push(Content::Text {
                text,
                cache_control: cache.then_some(CacheControl::Ephemeral),
            });
        }

        if !system_content.is_empty() {
            messages.push(Message::default().with_content(system_content).system());
        }

        // All attachments go in a user message before conversation events.
        let mut attachment_blocks = vec![];

        // Text attachments as XML to preserve source metadata.
        if let Some(xml) = text_attachments_to_xml(&attachments)? {
            attachment_blocks.push(Content::Text {
                text: xml,
                cache_control: None,
            });
        }

        // Binary attachments, each preceded by a label.
        for attachment in &attachments {
            if let AttachmentContent::Binary { data, media_type } = &attachment.content {
                let b64 = base64::engine::general_purpose::STANDARD.encode(data);

                attachment_blocks.push(Content::Text {
                    text: format!("[Attached file: {}]", attachment.source),
                    cache_control: None,
                });

                if media_type.starts_with("image/") {
                    attachment_blocks.push(Content::image_data_uri(format!(
                        "data:{media_type};base64,{b64}",
                    )));
                } else {
                    // OpenRouter accepts arbitrary files via its file content
                    // type. For models that don't support file input natively,
                    // OpenRouter parses the file server-side.
                    attachment_blocks.push(Content::File {
                        file: FilePayload {
                            filename: attachment.source.clone(),
                            file_data: format!("data:{media_type};base64,{b64}"),
                        },
                    });
                }
            }
        }

        if !attachment_blocks.is_empty() {
            messages.push(Message::default().with_content(attachment_blocks).user());
        }

        messages.extend(convert_events(events));

        // Only Anthropic and Google models support explicit caching.
        if !model_id.name.starts_with("anthropic") && !model_id.name.starts_with("google") {
            trace!(
                slug = %model_id.name,
                "Model does not support caching directives, disabling cache."
            );
            for m in &mut messages {
                m.content_mut().iter_mut().for_each(Content::disable_cache);
            }
        }

        Ok(RequestMessages(messages))
    }
}

/// Converts conversation events into OpenRouter request messages.
///
/// Expects a pre-filtered stream (internal events already removed by
/// [`Thread::into_parts`]).
fn convert_events(events: ConversationStream) -> Vec<RequestMessage> {
    let messages = events
        .into_iter()
        .flat_map(|event| convert_event_kind(event.event.kind))
        .collect();

    coalesce_assistant_messages(messages)
}

fn convert_conversation_events(events: Vec<ConversationEvent>) -> Vec<RequestMessage> {
    let messages = events
        .into_iter()
        .flat_map(|event| convert_event_kind(event.kind))
        .collect();

    coalesce_assistant_messages(messages)
}

/// Combine the separately stored parts of one assistant response.
///
/// Chat Completions requests require content and parallel tool calls from one
/// model response to share one assistant message.
/// Consecutive assistant messages can be interpreted as a trailing prefill by
/// Anthropic-compatible endpoints.
fn coalesce_assistant_messages(messages: Vec<RequestMessage>) -> Vec<RequestMessage> {
    messages
        .into_iter()
        .fold(Vec::new(), |mut messages, message| {
            match (messages.last_mut(), message) {
                (
                    Some(RequestMessage::Assistant(previous)),
                    RequestMessage::Assistant(mut next),
                ) => {
                    previous.content.append(&mut next.content);

                    if let Some(reasoning) = next.reasoning {
                        previous
                            .reasoning
                            .get_or_insert_default()
                            .push_str(&reasoning);
                    }

                    let first_index = previous.tool_calls.len();
                    previous
                        .tool_calls
                        .extend(next.tool_calls.into_iter().enumerate().map(
                            |(offset, mut tool_call)| {
                                tool_call.index = first_index + offset;
                                tool_call
                            },
                        ));
                }
                (_, message) => messages.push(message),
            }

            messages
        })
}

fn convert_event_kind(kind: EventKind) -> Vec<RequestMessage> {
    match kind {
        EventKind::ChatRequest(request) => {
            vec![Message::default().with_text(request.content).user()]
        }
        EventKind::ChatResponse(response) => match response {
            ChatResponse::Message { message } => {
                vec![Message::default().with_text(message).assistant()]
            }
            ChatResponse::Reasoning { reasoning, .. } => {
                vec![Message::default().with_reasoning(reasoning).assistant()]
            }
            ChatResponse::Structured { data } => {
                vec![Message::default().with_text(data.to_string()).assistant()]
            }
        },
        EventKind::ToolCallRequest(request) => {
            let message = Message {
                tool_calls: vec![ToolCall {
                    id: Some(request.id.clone()),
                    index: 0,
                    kind: ToolCallType::Function,
                    function: FunctionCall {
                        name: Some(request.name),
                        arguments: serde_json::to_string(&request.arguments).ok(),
                    },
                }],
                ..Default::default()
            };

            vec![message.assistant()]
        }
        EventKind::ToolCallResponse(response) => {
            let content = match response.result {
                Ok(content) => content,
                Err(error) => error,
            };

            vec![RequestMessage::Tool(tool::Message {
                tool_call_id: response.id,
                content,
                name: None,
            })]
        }
        // Internal events are filtered by into_parts(), but we still need
        // an exhaustive match.
        _ => vec![],
    }
}

impl From<jp_openrouter::Error> for StreamError {
    fn from(err: jp_openrouter::Error) -> Self {
        use jp_openrouter::Error as E;

        match err {
            E::Request(error) => Self::from(error),
            E::Api { code: 429, .. } => StreamError::rate_limit(None).with_source(err),
            // 402 Payment Required — OpenRouter returns this for insufficient credits.
            E::Api { code: 402, .. } => StreamError::new(
                StreamErrorKind::InsufficientQuota,
                format!(
                    "Insufficient API quota. Check your credits \
                     at https://openrouter.ai/settings/credits. ({err})"
                ),
            )
            .with_source(err),
            E::Api { ref message, .. } if looks_like_quota_error(message) => StreamError::new(
                StreamErrorKind::InsufficientQuota,
                format!(
                    "Insufficient API quota. Check your credits \
                     at https://openrouter.ai/settings/credits. ({err})"
                ),
            )
            .with_source(err),
            E::Api {
                code: 408 | 500 | 502 | 503 | 504,
                ..
            }
            | E::Stream(_) => StreamError::transient(err.to_string()).with_source(err),
            _ => StreamError::other(err.to_string()).with_source(err),
        }
    }
}

#[cfg(test)]
#[path = "openrouter_tests.rs"]
mod tests;
