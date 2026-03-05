use std::{env, future, time::Duration};

use async_anthropic::{
    Client,
    errors::AnthropicError,
    messages::DEFAULT_MAX_TOKENS,
    types::{
        self, Effort, JsonOutputFormat, ListModelsResponse, OutputConfig, System, Thinking,
        ToolBash, ToolCodeExecution, ToolComputerUse, ToolTextEditor, ToolWebSearch,
    },
};
use async_stream::try_stream;
use async_trait::async_trait;
use chrono::NaiveDate;
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _, pin_mut, stream};
use indexmap::IndexMap;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{Name, ProviderId},
        parameters::ReasoningEffort,
    },
    providers::llm::anthropic::AnthropicConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, ConversationEvent, EventKind, ToolCallRequest},
    thread::{Document, Documents, Thread},
};
use serde_json::{Map, Value, json};
use tracing::{debug, info, trace, warn};

use super::Provider;
use crate::{
    error::{
        Error, Result, StreamError, StreamErrorKind, extract_retry_from_text,
        looks_like_quota_error,
    },
    event::{Event, FinishReason},
    model::{ModelDeprecation, ModelDetails, ReasoningDetails},
    query::ChatQuery,
    stream::{
        EventStream,
        aggregator::tool_call_request::{AggregationError, ToolCallRequestAggregator},
    },
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Anthropic;

/// Anthropic limits the number of cache points to 4 per request. Returning an API error if the
/// request exceeds this limit.
///
/// We detect where we inject cache controls and make sure to not exceed this limit.
const MAX_CACHE_CONTROL_COUNT: usize = 4;

const THINKING_SIGNATURE_KEY: &str = "anthropic_thinking_signature";
const REDACTED_THINKING_KEY: &str = "anthropic_redacted_thinking";

/// Known Anthropic error types that are safe to retry.
///
/// See: <https://docs.claude.com/en/docs/build-with-claude/streaming#error-events>
/// See: <https://docs.claude.com/en/api/errors#http-errors>
const RETRYABLE_ANTHROPIC_ERROR_TYPES: &[&str] =
    &["rate_limit_error", "overloaded_error", "api_error"];

/// Supported string `format` values for Anthropic structured output.
const SUPPORTED_STRING_FORMATS: &[&str] = &[
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];

#[derive(Debug, Clone)]
pub struct Anthropic {
    client: Client,

    /// See [`AnthropicConfig::chain_on_max_tokens`].
    chain_on_max_tokens: bool,

    /// Which beta features are enabled.
    beta: BetaFeatures,
}

#[async_trait]
impl Provider for Anthropic {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails> {
        let model = self.client.models().get(name).await?;
        map_model(model, &self.beta)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>> {
        let mut all_models = vec![];
        let mut after_id = None;

        loop {
            let path = match after_id {
                Some(id) => format!("/v1/models?after_id={id}"),
                None => "/v1/models".to_string(),
            };

            let models;
            (after_id, models) = self
                .client
                .get::<ListModelsResponse>(&path)
                .await
                .map(|list| (list.has_more.then_some(list.last_id).flatten(), list.data))?;

            all_models.extend(models);

            if after_id.is_none() {
                break;
            }
        }

        all_models
            .into_iter()
            .map(|v| map_model(v, &self.beta))
            .collect::<Result<_>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream> {
        let client = self.client.clone();
        let max_tokens_config = query
            .thread
            .events
            .config()?
            .assistant
            .model
            .parameters
            .max_tokens;

        let (request, is_structured) = create_request(model, query, true, &self.beta)?;

        // Chaining is disabled for structured output — the provider guarantees
        // schema compliance so the response won't hit max_tokens for a
        // well-constrained schema.
        //
        // It is also disabled when the user has explicitly configured a max
        // tokens value, or when chaining is disabled in the provider config.
        let chain_on_max_tokens =
            !is_structured && max_tokens_config.is_none() && self.chain_on_max_tokens;

        debug!(stream = true, "Anthropic chat completion stream request.");
        trace!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            "Request payload."
        );

        Ok(call(client, request, chain_on_max_tokens, is_structured))
    }
}

/// Create a request to the assistant to generate a response, and return a
/// stream of [`Event`]s.
///
/// If `chain_on_max_tokens` is `true`, a new request is created when the last
/// one ends with a [`FinishReason::MaxTokens`] event, allowing the assistant to
/// continue from where it left off and the caller to receive the full response
/// as a single stream of events.
fn call(
    client: Client,
    request: types::CreateMessagesRequest,
    chain_on_max_tokens: bool,
    is_structured: bool,
) -> EventStream {
    Box::pin(try_stream!({
        let mut tool_call_aggregator = ToolCallRequestAggregator::new();
        let mut events = vec![];

        // If a tool call is requested, we cannot chain on max tokens, as the
        // Anthropic API expects the response to a tool call to contain the tool
        // result.
        let mut tool_calls_requested = false;

        let stream = client
            .messages()
            .create_stream(request.clone())
            .await
            .map_err(StreamError::from)
            .map_ok(|v| stream::iter(map_event(v, &mut tool_call_aggregator, is_structured)))
            .try_flatten()
            .chain(future::ready(Ok(Event::Finished(FinishReason::Completed))).into_stream())
            .peekable();

        pin_mut!(stream);
        while let Some(event) = stream.next().await.transpose()? {
            match event {
                // If the assistant has reached the maximum number of
                // tokens, and we are in a state in which we can request
                // more tokens, we do so by sending a new request and
                // chaining those events onto the previous ones, keeping the
                // existing stream of events alive.
                //
                // TODO: generalize this for any provider.
                event if should_chain(&event, tool_calls_requested, chain_on_max_tokens) => {
                    debug!("Max tokens reached, auto-requesting more tokens.");

                    for await event in chain(client.clone(), request.clone(), events, is_structured)
                    {
                        yield event?;
                    }
                    return;
                }
                done @ Event::Finished(_) => {
                    yield done;
                    return;
                }
                Event::Part { event, index } => {
                    if event.is_tool_call_request() {
                        tool_calls_requested = true;
                    } else if chain_on_max_tokens {
                        events.push(event.clone());
                    }

                    yield Event::Part { event, index };
                }
                flush @ Event::Flush { .. } => {
                    let next_event = stream.as_mut().peek().await.and_then(|e| e.as_ref().ok());

                    // If we try to flush, but we're about to continue with a
                    // chained request, we ignore the flush event, to allow more
                    // event parts to be generated by the next response.
                    if let Some(event) = next_event
                        && should_chain(event, tool_calls_requested, chain_on_max_tokens)
                    {
                        continue;
                    }

                    yield flush;
                }
            }
        }

        yield Event::Finished(FinishReason::Completed);
    }))
}

/// Check if we should chain more events from a new request.
fn should_chain(event: &Event, tool_calls_requested: bool, chain_on_max_tokens: bool) -> bool {
    !tool_calls_requested
        && chain_on_max_tokens
        && matches!(event, Event::Finished(FinishReason::MaxTokens))
}

/// Create a new `EventStream` by asking the assistant to continue from where it
/// left off.
fn chain(
    client: Client,
    mut request: types::CreateMessagesRequest,
    events: Vec<ConversationEvent>,
    is_structured: bool,
) -> EventStream {
    debug_assert!(!events.iter().any(ConversationEvent::is_tool_call_request));

    let mut should_merge = true;
    let previous_content = events
        .last()
        .and_then(|e| match e.as_chat_response() {
            Some(ChatResponse::Message { message }) => Some(message.as_str()),
            Some(ChatResponse::Reasoning { reasoning }) => Some(reasoning.as_str()),
            _ => None,
        })
        .unwrap_or_default()
        .to_owned();

    let message = events
        .into_iter()
        .filter_map(|event| convert_event(event, true).map(|v| v.1))
        .fold(
            types::Message {
                role: types::MessageRole::Assistant,
                ..Default::default()
            },
            |mut message, content| {
                message.content.push(content);
                message
            },
        );

    request.messages.push(message);
    request.messages.push(types::Message {
        role: types::MessageRole::User,
        content: types::MessageContentList(vec![types::MessageContent::Text(
            "Please continue from where you left off. DO NOT reason or mention being asked to \
             continue the conversation, just do it. YOU MUST repeat between 10 and 100 characters \
             from the end of the previous message, so that our system can find the correct merge \
             point between the two messages."
                .into(),
        )]),
    });

    Box::pin(try_stream!({
        for await event in call(client, request, true, is_structured) {
            let mut event = event?;

            // When chaining new events, the reasoning content is irrelevant, as
            // it will contain text such as "the user asked me to continue
            // [...]".
            //
            // NOTE: we should never end up in a situation in which we want to
            // continue an existing reasoning stint from the previous request,
            // as the model is configured to reason with less tokens than the
            // total maximum number of tokens allowed, so we do not need to
            // worry about omitting valid reasoning content here.
            if event
                .as_conversation_event()
                .and_then(ConversationEvent::as_chat_response)
                .is_some_and(ChatResponse::is_reasoning)
            {
                continue;
            }

            // Merge the new content with the previous content, if there is any
            // overlap. Sometimes the assistant will start a chaining response
            // with a small amount of content that was already seen in the
            // previous response, and we want to avoid duplicating that.
            if let Some(
                ChatResponse::Message { message: content }
                | ChatResponse::Reasoning { reasoning: content },
            ) = event
                .as_conversation_event_mut()
                .and_then(ConversationEvent::as_chat_response_mut)
            {
                if should_merge {
                    let merge_point = find_merge_point(&previous_content, content, 500);
                    content.replace_range(..merge_point, "");
                }

                // After receiving the first content event, we can
                // stop merging.
                should_merge = false;
            }

            yield event;
        }

        return;
    }))
}

/// Finds the merge point between two text chunks by detecting overlapping
/// content.
///
/// Returns the number of bytes to skip from the start of `right` to merge it
/// seamlessly with `left`.
fn find_merge_point(left: &str, right: &str, max_search: usize) -> usize {
    const MIN_OVERLAP: usize = 5;

    let max_overlap = left.len().min(right.len()).min(max_search);

    // Try progressively smaller overlaps, but stop at minimum threshold
    for overlap in (MIN_OVERLAP..=max_overlap).rev() {
        let left_start = left.len() - overlap;

        // Only attempt comparison if both positions are valid UTF-8 char
        // boundaries
        if left.is_char_boundary(left_start) && right.is_char_boundary(overlap) {
            let left_suffix = &left[left_start..];
            let right_prefix = &right[..overlap];

            if left_suffix == right_prefix {
                return overlap;
            }
        }
    }

    // No overlap found (or overlap was below minimum threshold)
    0
}

#[derive(Debug, Clone, Default)]
struct BetaFeatures(Vec<String>);

impl BetaFeatures {
    /// See: <https://docs.claude.com/en/docs/build-with-claude/extended-thinking#interleaved-thinking>
    fn interleaved_thinking(&self) -> bool {
        self.0
            .iter()
            .any(|h| h == "interleaved-thinking-2025-05-14")
    }

    /// See: <https://docs.claude.com/en/docs/build-with-claude/context-editing>
    fn context_editing(&self) -> bool {
        self.0.iter().any(|h| h == "context-editing-2025-06-27")
    }

    /// See: <https://docs.claude.com/en/api/rate-limits#long-context-rate-limits>
    fn context_1m(&self) -> bool {
        self.0.iter().any(|h| h == "context-1m-2025-08-07")
    }

    /// See: <https://platform.claude.com/docs/en/build-with-claude/structured-outputs>
    fn structured_outputs(&self) -> bool {
        self.0.iter().any(|h| h == "structured-outputs-2025-10-27")
    }
}

#[expect(clippy::too_many_lines)]
fn create_request(
    model: &ModelDetails,
    query: ChatQuery,
    stream: bool,
    beta: &BetaFeatures,
) -> Result<(types::CreateMessagesRequest, bool)> {
    let ChatQuery {
        thread,
        tools,
        mut tool_choice,
    } = query;

    let mut builder = types::CreateMessagesRequestBuilder::default();

    builder.stream(stream);

    let Thread {
        system_prompt,
        sections,
        attachments,
        events,
    } = thread;

    // Request a structured response if the very last event is a ChatRequest
    // with a schema attached. The schema is transformed to strip unsupported
    // properties (moving them into `description` fields as hints).
    let format = events
        .last()
        .and_then(|e| e.event.as_chat_request())
        .and_then(|req| req.schema.clone())
        .map(|schema| JsonOutputFormat::JsonSchema {
            schema: transform_schema(schema),
        });

    let mut cache_control_count = MAX_CACHE_CONTROL_COUNT;
    let config = events.config()?;

    builder
        .model(model.id.name.clone())
        .messages(AnthropicMessages::build(events, &mut cache_control_count).0);

    let strict_tools = model.features.contains(&"structured-outputs") && beta.structured_outputs();
    let tools = convert_tools(tools, strict_tools, &mut cache_control_count);

    let mut system_content = vec![];

    if let Some(text) = system_prompt {
        system_content.push(types::SystemContent::Text(types::Text {
            text,
            cache_control: (cache_control_count > 0).then_some({
                cache_control_count = cache_control_count.saturating_sub(1);
                types::CacheControl::default()
            }),
        }));
    }

    // FIXME: Somehow the system_prompt is being duplicated. It has to do with
    // `impl PartialConfigDelta`?
    // dbg!(&system_content);

    if !sections.is_empty() {
        // Each section gets its own system content block. Cache control
        // is placed on the last section, as section content is unlikely
        // to change between requests.
        let mut sections = sections.iter().peekable();
        while let Some(section) = sections.next() {
            system_content.push(types::SystemContent::Text(types::Text {
                text: section.render(),
                cache_control: sections.peek().map_or_else(
                    || {
                        (cache_control_count > 0).then(|| {
                            cache_control_count = cache_control_count.saturating_sub(1);
                            types::CacheControl::default()
                        })
                    },
                    |_| None,
                ),
            }));
        }
    }

    if !attachments.is_empty() {
        let documents: Documents = attachments
            .into_iter()
            .enumerate()
            .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
            .map(Document::from)
            .collect::<Vec<_>>()
            .into();

        system_content.push(types::SystemContent::Text(types::Text {
            text: documents.try_to_xml()?,

            // Anthropic limits the number of cache points to 4 per request. We
            // currently use 5 cache points, so this one is optional, depending
            // on whether we have any tools or not, making sure we stay within
            // the limit.
            cache_control: (cache_control_count > 0).then_some({
                _ = cache_control_count.saturating_sub(1);
                types::CacheControl::default()
            }),
        }));
    }

    // From testing, it seems that sending a single tool with the
    // "function" tool choice can result in incorrect API responses from
    // Anthropic. I (Jean) have an open support case with Anthropic to dig into
    // this finding more.
    if tools.len() == 1 && matches!(tool_choice, ToolChoice::Function(_)) {
        tool_choice = ToolChoice::Required;
    }

    let parameters = &config.assistant.model.parameters;
    let max_tokens = parameters
        .max_tokens
        .or(model.max_output_tokens)
        .unwrap_or_else(|| {
            warn!(
                %model.id,
                %DEFAULT_MAX_TOKENS,
                "Model `max_tokens` parameter not found, using default value."
            );

            DEFAULT_MAX_TOKENS as u32
        });

    let reasoning_config = model.custom_reasoning_config(parameters.reasoning);

    // See: <https://docs.claude.com/en/docs/build-with-claude/extended-thinking#extended-thinking-with-tool-use>
    if reasoning_config.is_some() && tool_choice.is_forced_call() {
        info!(
            ?tool_choice,
            "Anthropic API does not support reasoning when tool_choice forces tool use. Switching \
             to soft-force mode."
        );
        tool_choice = ToolChoice::Auto;
        system_content.push(types::SystemContent::Text(types::Text {
            text: {
                let msg = "IMPORTANT:";
                let msg = if let Some(tool) = tool_choice.function_name() {
                    format!("{msg} You MUST use the function named '{tool}' available to you.")
                } else {
                    format!("{msg} You MUST use AT LEAST ONE tool available to you.")
                };

                format!(
                    "{msg} DO NOT QUESTION THIS DIRECTIVE. DO NOT PROMPT FOR MORE CONTEXT OR \
                     DETAILS. JUST RUN IT."
                )
            },
            cache_control: None,
        }));
    }

    let tool_choice = convert_tool_choice(tool_choice);

    if !tools.is_empty() {
        builder.tools(tools).tool_choice(tool_choice);
    }

    if !system_content.is_empty() {
        builder.system(System::Content(system_content));
    }

    // Track the effort from reasoning config so we can merge it with the
    // structured output format into a single OutputConfig at the end.
    let mut effort = None;
    let supports_thinking = model.reasoning.is_some_and(|r| !r.is_unsupported());

    if let Some(config) = reasoning_config {
        match model.reasoning {
            // Adaptive thinking for Opus 4.6+
            Some(ReasoningDetails::Adaptive { max: supports_max }) => {
                builder.thinking(types::ExtendedThinking::Adaptive);

                effort = match config
                    .effort
                    .abs_to_rel(model.max_output_tokens)
                    .unwrap_or(ReasoningEffort::Auto)
                {
                    ReasoningEffort::Max if supports_max => Some(Effort::Max),
                    ReasoningEffort::Max
                    | ReasoningEffort::XHigh
                    | ReasoningEffort::High
                    | ReasoningEffort::Absolute(_) => Some(Effort::High),
                    ReasoningEffort::Medium => Some(Effort::Medium),
                    ReasoningEffort::Low | ReasoningEffort::Xlow | ReasoningEffort::None => {
                        Some(Effort::Low)
                    }
                    ReasoningEffort::Auto => None,
                };
            }

            // Budget-based thinking for older models
            Some(ReasoningDetails::Budgetted {
                min_tokens,
                max_tokens: reasoning_max_tokens,
            }) => {
                let mut max_budget = reasoning_max_tokens.unwrap_or(u32::MAX);

                // With interleaved thinking, the `budget_tokens` can exceed the
                // `max_tokens` parameter, as it represents the total budget across all
                // thinking blocks within one assistant turn.
                //
                // See: <https://docs.claude.com/en/docs/build-with-claude/extended-thinking#interleaved-thinking>
                //
                // This is only enabled if the model supports it, otherwise an error is
                // returned if the `max_tokens` parameter is larger than the model's
                // supported range.
                if beta.interleaved_thinking() && model.features.contains(&"interleaved-thinking") {
                    max_budget = model.context_window.unwrap_or(max_budget);
                }

                builder.thinking(types::ExtendedThinking::Enabled {
                    budget_tokens: config
                        .effort
                        .to_tokens(max_tokens)
                        .max(min_tokens)
                        .min(max_budget),
                });
            }

            // Other reasoning details (Leveled, Unsupported) - no thinking config
            _ => {}
        }
    } else if supports_thinking {
        // Reasoning is off but the model supports it — explicitly disable
        // to prevent the model from thinking by default.
        builder.thinking(types::ExtendedThinking::Disabled);
    }

    let is_structured = format.is_some();
    if effort.is_some() || is_structured {
        builder.output_config(OutputConfig { effort, format });
    }

    if let Some(temperature) = parameters.temperature {
        builder.temperature(temperature);
    }

    #[expect(clippy::cast_possible_wrap)]
    builder.max_tokens(max_tokens as i32);

    if let Some(top_p) = parameters.top_p {
        builder.top_p(top_p);
    }

    if let Some(top_k) = parameters.top_k {
        builder.top_k(top_k);
    }

    // See: <https://docs.claude.com/en/docs/build-with-claude/context-editing>
    if beta.context_editing() {
        let strategy = match parameters.other.get("context_management").cloned() {
            Some(Value::Object(strategy)) => strategy,
            // If no strategy is provided, but the `context_editing` feature is
            // enabled, use the default strategy.
            _ => Map::from_iter([(
                "edits".into(),
                json!([{"type": "clear_tool_uses_20250919"}]),
            )]),
        };

        builder.context_management(strategy);
    }

    builder
        .build()
        .map(|req| (req, is_structured))
        .map_err(Into::into)
}

#[expect(clippy::too_many_lines)]
fn map_model(model: types::Model, beta: &BetaFeatures) -> Result<ModelDetails> {
    let details = match model.id.as_str() {
        "claude-sonnet-4-6" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: if beta.context_1m() {
                Some(1_000_000)
            } else {
                Some(200_000)
            },
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::adaptive(true)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![
                "interleaved-thinking",
                "context-editing",
                "structured-outputs",
                "adaptive-thinking",
            ],
        },
        "claude-opus-4-6" | "claude-opus-4-6-20260205" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: if beta.context_1m() {
                Some(1_000_000)
            } else {
                Some(200_000)
            },
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::adaptive(true)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 5, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![
                "interleaved-thinking",
                "context-editing",
                "structured-outputs",
                "adaptive-thinking",
            ],
        },
        "claude-opus-4-5" | "claude-opus-4-5-20251101" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 7, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![
                "interleaved-thinking",
                "context-editing",
                "structured-outputs",
            ],
        },
        "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 7, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec!["interleaved-thinking", "context-editing"],
        },
        "claude-sonnet-4-5" | "claude-sonnet-4-5-20250929" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: if beta.context_1m() {
                Some(1_000_000)
            } else {
                Some(200_000)
            },
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 7, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![
                "interleaved-thinking",
                "context-editing",
                "structured-outputs",
            ],
        },
        "claude-opus-4-1" | "claude-opus-4-1-20250805" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(32_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 3, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![
                "interleaved-thinking",
                "context-editing",
                "structured-outputs",
            ],
        },
        "claude-opus-4-0" | "claude-opus-4-20250514" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(32_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 3, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec!["interleaved-thinking", "context-editing"],
        },
        "claude-sonnet-4-0" | "claude-sonnet-4-20250514" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: if beta.context_1m() {
                Some(1_000_000)
            } else {
                Some(200_000)
            },
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 3, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec!["interleaved-thinking", "context-editing"],
        },
        "claude-3-haiku-20240307" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 8, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: claude-haiku-4-5-20251001",
                Some(NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()),
            )),
            features: vec![],
        },
        id => {
            debug!(model = id, ?model, "Missing model details.");
            let mut model = ModelDetails::empty((PROVIDER, id).try_into()?);
            model.display_name = Some(id.to_string());
            model
        }
    };

    Ok(details)
}

fn map_event(
    event: types::MessagesStreamEvent,
    agg: &mut ToolCallRequestAggregator,
    is_structured: bool,
) -> Vec<std::result::Result<Event, StreamError>> {
    use types::MessagesStreamEvent::*;

    trace!(
        event = serde_json::to_string(&event).unwrap_or_default(),
        "Received event from Anthropic API."
    );

    match event {
        ContentBlockStart {
            content_block,
            index,
        } => map_content_start(content_block, index, agg, is_structured)
            .into_iter()
            .map(Ok)
            .collect(),
        ContentBlockDelta { delta, index } => map_content_delta(delta, index, agg, is_structured)
            .into_iter()
            .map(Ok)
            .collect(),
        ContentBlockStop { index } => map_content_stop(index, agg),
        MessageDelta { delta, .. } => map_message_delta(&delta).into_iter().map(Ok).collect(),
        _ => vec![],
    }
}

impl TryFrom<&AnthropicConfig> for Anthropic {
    type Error = Error;

    fn try_from(config: &AnthropicConfig) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        let mut builder = Client::builder();
        builder
            .api_key(api_key)
            .base_url(config.base_url.clone())
            .version("2023-06-01");

        if !config.beta_headers.is_empty() {
            builder.beta(config.beta_headers.join(","));
        }

        Ok(Anthropic {
            beta: BetaFeatures(config.beta_headers.clone()),
            chain_on_max_tokens: config.chain_on_max_tokens,
            client: builder
                .build()
                .map_err(|e| Error::Anthropic(AnthropicError::Unknown(e.to_string())))?,
        })
    }
}

/// Transform a JSON schema to conform to Anthropic's structured output
/// constraints.
///
/// Anthropic's structured output supports a subset of JSON Schema. Unsupported
/// properties are stripped and appended to the `description` field so the model
/// can still see them as soft hints.
///
/// Mirrors the logic from Anthropic's Python SDK `transform_schema`.
///
/// See: <https://docs.claude.com/en/docs/build-with-claude/structured-outputs#json-schema-limitations>
fn transform_schema(mut src: Map<String, Value>) -> Map<String, Value> {
    if let Some(r) = src.remove("$ref") {
        return Map::from_iter([("$ref".into(), r)]);
    }

    let mut out = Map::new();

    // Helper macro to move a field from src to out.
    macro_rules! move_field {
        ($key:literal) => {
            if let Some(v) = src.remove($key) {
                out.insert($key.into(), v);
            }
        };
    }

    // Extract common fields
    move_field!("title");
    move_field!("description");

    // Recursive Transformation Helpers
    let transform_val = |v: Value| match v {
        Value::Object(o) => Value::Object(transform_schema(o)),
        other => other,
    };

    let transform_map = |m: Map<String, Value>| -> Map<String, Value> {
        m.into_iter().map(|(k, v)| (k, transform_val(v))).collect()
    };

    let transform_vec =
        |v: Vec<Value>| -> Vec<Value> { v.into_iter().map(transform_val).collect() };

    // Handle Recursive Dictionaries
    for key in ["$defs", "definitions"] {
        if let Some(Value::Object(defs)) = src.remove(key) {
            out.insert(key.into(), Value::Object(transform_map(defs)));
        }
    }

    // Handle Combinators
    if let Some(Value::Array(variants)) = src.remove("anyOf") {
        out.insert("anyOf".into(), Value::Array(transform_vec(variants)));
    } else if let Some(Value::Array(variants)) = src.remove("oneOf") {
        // Remap oneOf -> anyOf
        out.insert("anyOf".into(), Value::Array(transform_vec(variants)));
    } else if let Some(Value::Array(variants)) = src.remove("allOf") {
        out.insert("allOf".into(), Value::Array(transform_vec(variants)));
    }

    // Handle Type-Specific Logic
    //
    // We remove "type" now so it doesn't get caught in the "leftovers" logic
    // later.
    let type_val = src.remove("type");
    match type_val.as_ref().and_then(Value::as_str) {
        Some("object") => {
            if let Some(Value::Object(props)) = src.remove("properties") {
                out.insert("properties".into(), Value::Object(transform_map(props)));
            }

            move_field!("required");

            // Force strictness
            src.remove("additionalProperties");
            out.insert("additionalProperties".into(), Value::Bool(false));
        }
        Some("array") => {
            if let Some(items) = src.remove("items") {
                out.insert("items".into(), transform_val(items));
            }

            // Enforce minItems logic
            if let Some(min) = src.remove("minItems") {
                if min.as_u64().is_some_and(|n| n <= 1) {
                    out.insert("minItems".into(), min);
                } else {
                    // Put it back into src so it falls into description
                    src.insert("minItems".into(), min);
                }
            }
        }
        Some("string") => {
            if let Some(format) = src.remove("format") {
                let is_supported = format
                    .as_str()
                    .is_some_and(|f| SUPPORTED_STRING_FORMATS.contains(&f));

                if is_supported {
                    out.insert("format".into(), format);
                } else {
                    src.insert("format".into(), format);
                }
            }
        }
        _ => {}
    }

    // Re-insert type.
    if let Some(t) = type_val {
        out.insert("type".into(), t);
    }

    // 7. Handle "Leftovers" (Unsupported fields -> Description)
    if !src.is_empty() {
        let extra_info = src
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(", ");

        out.entry("description")
            .and_modify(|v| {
                if let Some(s) = v.as_str() {
                    *v = Value::from(format!("{s}\n\n{{{extra_info}}}"));
                }
            })
            .or_insert_with(|| Value::from(format!("{{{extra_info}}}")));
    }

    out
}

fn convert_tool_choice(choice: ToolChoice) -> types::ToolChoice {
    match choice {
        ToolChoice::None => types::ToolChoice::none(),
        ToolChoice::Auto => types::ToolChoice::auto(),
        ToolChoice::Required => types::ToolChoice::any(),
        ToolChoice::Function(name) => types::ToolChoice::tool(name),
    }
}

fn convert_tools(
    tools: Vec<ToolDefinition>,
    strict: bool,
    cache_controls: &mut usize,
) -> Vec<types::Tool> {
    let mut tools: Vec<_> = tools
        .into_iter()
        .map(|tool| {
            types::Tool::Custom(types::CustomTool {
                name: tool.name,
                description: tool.description,
                strict: strict.then_some(true),
                input_schema: {
                    let required = tool
                        .parameters
                        .iter()
                        .filter(|(_, cfg)| cfg.required)
                        .map(|(key, _)| key.clone())
                        .collect();

                    let properties = tool
                        .parameters
                        .into_iter()
                        .map(|(key, cfg)| (key, cfg.to_json_schema()))
                        .collect();

                    types::ToolInputSchema {
                        kind: types::ToolInputSchemaKind::Object,
                        properties,
                        required,
                        additional_properties: strict.then_some(false),
                    }
                },
                cache_control: None,
            })
        })
        .collect();

    // Cache tool definitions, as they are unlikely to change.
    if *cache_controls > 0
        && let Some(tool) = tools.last_mut()
    {
        let cache_control = match tool {
            types::Tool::Custom(tool) => &mut tool.cache_control,
            types::Tool::Bash(ToolBash::Bash20241022(tool)) => &mut tool.cache_control,
            types::Tool::Bash(ToolBash::Bash20250124(tool)) => &mut tool.cache_control,
            types::Tool::CodeExecution(ToolCodeExecution::CodeExecution20250522(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::ComputerUse(ToolComputerUse::ComputerUse20241022(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::ComputerUse(ToolComputerUse::ComputerUse20250124(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20241022(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20250124(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20250429(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::WebSearch(ToolWebSearch::WebSearch20250305(tool)) => {
                &mut tool.cache_control
            }
        };

        *cache_control = Some(types::CacheControl::default());
        *cache_controls = cache_controls.saturating_sub(1);
    }

    tools
}

struct AnthropicMessages(Vec<types::Message>);

impl AnthropicMessages {
    fn build(events: ConversationStream, cache_controls: &mut usize) -> Self {
        let mut messages = convert_events(events);

        // Make sure to add cache control to the last history message.
        if *cache_controls > 0
            && let Some(message) = messages.last_mut().and_then(|m| m.content.0.last_mut())
        {
            *cache_controls = cache_controls.saturating_sub(1);

            match message {
                types::MessageContent::Text(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                types::MessageContent::ToolUse(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                types::MessageContent::ToolResult(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                _ => {}
            }
        }

        Self(messages)
    }
}

/// Groups consecutive events into messages by role.
///
/// Events from the same role are combined into a single message with multiple
/// content blocks.
fn convert_events(events: ConversationStream) -> Vec<types::Message> {
    events
        .into_iter()
        .filter_map(|event| {
            // FIXME: `aliases` is empty here, because of an issue in `query.rs`
            // where we merge different configs... It has to do with us using
            // `PartialAppConfig::empty()` as a base config there.
            let aliases = &event.config.providers.llm.aliases;

            // dbg!(&aliases);
            // dbg!(&event.config.assistant.model.id);

            let is_anthropic = event
                .config
                .assistant
                .model
                .id
                .finalize(aliases)
                .is_ok_and(|id| id.provider == Some(PROVIDER));

            convert_event(event.event, is_anthropic)
        })
        .fold(vec![], |mut messages, (role, content)| {
            match messages.last_mut() {
                // If the last message has the same role, append content to it.
                Some(last) if last.role == role => last.content.0.push(content),
                // Different role or no messages yet, start a new message.
                _ => messages.push(types::Message {
                    role,
                    content: types::MessageContentList(vec![content]),
                }),
            }

            messages
        })
}

fn convert_event(
    event: ConversationEvent,
    is_anthropic: bool,
) -> Option<(types::MessageRole, types::MessageContent)> {
    let ConversationEvent { kind, metadata, .. } = event;

    match kind {
        EventKind::ChatRequest(request) if !request.content.is_empty() => Some((
            types::MessageRole::User,
            types::MessageContent::Text(request.content.into()),
        )),
        EventKind::ChatResponse(response) => {
            // Check if this came from Anthropic originally
            // dbg!(&is_anthropic);
            // dbg!(&metadata);

            let content = if is_anthropic
                && response.is_reasoning()
                && let signature = metadata
                    .get(THINKING_SIGNATURE_KEY)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                && signature.is_some()
            {
                types::MessageContent::Thinking(Thinking {
                    thinking: match response {
                        ChatResponse::Reasoning { reasoning } => reasoning,
                        ChatResponse::Message { message } => message,
                        ChatResponse::Structured { data } => data.to_string(),
                    },
                    signature,
                })
            } else if is_anthropic
                && response.is_reasoning()
                && let Some(data) = metadata
                    .get(REDACTED_THINKING_KEY)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            {
                types::MessageContent::RedactedThinking { data }
            } else {
                match response {
                    // Reasoning from other providers - wrap in <think> tags.
                    ChatResponse::Reasoning { reasoning } => types::MessageContent::Text(
                        format!("<think>\n{reasoning}\n</think>\n\n").into(),
                    ),
                    ChatResponse::Message { message } => {
                        types::MessageContent::Text(message.into())
                    }
                    ChatResponse::Structured { data } => {
                        types::MessageContent::Text(data.to_string().into())
                    }
                }
            };

            Some((types::MessageRole::Assistant, content))
        }
        EventKind::ToolCallRequest(request) => Some((
            types::MessageRole::Assistant,
            types::MessageContent::ToolUse(types::ToolUse {
                id: request.id,
                name: request.name,
                input: Value::Object(request.arguments),
                cache_control: None,
            }),
        )),
        EventKind::ToolCallResponse(response) => {
            let (content, is_error) = match response.result {
                Ok(c) => (Some(c), false),
                Err(e) => (Some(e), true),
            };

            Some((
                types::MessageRole::User,
                types::MessageContent::ToolResult(types::ToolResult {
                    tool_use_id: response.id,
                    content,
                    is_error,
                    cache_control: None,
                }),
            ))
        }
        EventKind::ChatRequest(_)
        | EventKind::InquiryRequest(_)
        | EventKind::InquiryResponse(_)
        | EventKind::TurnStart(_) => None,
    }
}

fn map_content_start(
    item: types::MessageContent,
    index: usize,
    agg: &mut ToolCallRequestAggregator,
    is_structured: bool,
) -> Option<Event> {
    use types::MessageContent::*;

    let mut metadata = IndexMap::new();
    let kind: EventKind = match item {
        // Initial part indicating a tool call request has started. The eventual
        // fully-aggregated arguments will be sent in a separate Part.
        ToolUse(types::ToolUse { id, name, .. }) => {
            *agg = ToolCallRequestAggregator::default();
            agg.add_chunk(index, Some(id.clone()), Some(name.clone()), None);
            let request = ToolCallRequest {
                id,
                name,
                arguments: Map::new(),
            };

            return Some(Event::Part {
                index,
                event: request.into(),
            });
        }
        Text(text) if is_structured => ChatResponse::structured(Value::String(text.text)).into(),
        Text(text) if !text.text.is_empty() => ChatResponse::message(text.text).into(),
        Text(_) => return None,
        Thinking(types::Thinking {
            thinking,
            signature,
        }) => {
            if let Some(signature) = signature
                && !signature.is_empty()
            {
                metadata.insert(THINKING_SIGNATURE_KEY.to_owned(), signature.into());
            }

            ChatResponse::reasoning(thinking).into()
        }
        RedactedThinking { data } => {
            metadata.insert(REDACTED_THINKING_KEY.to_owned(), data.into());
            ChatResponse::reasoning("").into()
        }
        ToolResult(_) => unreachable!("never triggered by the API"),
    };

    Some(Event::Part {
        event: ConversationEvent::now(kind).with_metadata(metadata),
        index,
    })
}

fn map_content_delta(
    delta: types::ContentBlockDelta,
    index: usize,
    agg: &mut ToolCallRequestAggregator,
    is_structured: bool,
) -> Option<Event> {
    let mut metadata = IndexMap::new();
    let kind: EventKind = match delta {
        types::ContentBlockDelta::TextDelta { text } if is_structured => {
            ChatResponse::structured(Value::String(text)).into()
        }
        types::ContentBlockDelta::TextDelta { text } => ChatResponse::message(text).into(),
        types::ContentBlockDelta::ThinkingDelta { thinking } => {
            ChatResponse::reasoning(thinking).into()
        }
        // This is only used for thinking blocks, and we need to store this
        // signature to pass it back to the assistant in the message
        // history.
        //
        // See: <https://docs.anthropic.com/en/docs/build-with-claude/streaming#thinking-delta>
        types::ContentBlockDelta::SignatureDelta { signature } => {
            metadata.insert(THINKING_SIGNATURE_KEY.to_owned(), signature.into());
            ChatResponse::reasoning("").into()
        }
        types::ContentBlockDelta::InputJsonDelta { partial_json } => {
            agg.add_chunk(index, None, None, Some(&partial_json));
            return None;
        }
    };

    Some(Event::Part {
        event: ConversationEvent::now(kind).with_metadata(metadata),
        index,
    })
}

fn map_content_stop(
    index: usize,
    agg: &mut ToolCallRequestAggregator,
) -> Vec<std::result::Result<Event, StreamError>> {
    let mut events = vec![];

    // Check if we're buffering a tool call request
    match agg.finalize(index) {
        Ok(tool_call) => events.push(Ok(Event::Part {
            event: ConversationEvent::now(tool_call),
            index,
        })),
        Err(AggregationError::UnknownIndex) => {}
        Err(error) => {
            events.push(Err(StreamError::other(error.to_string())));
            return events;
        }
    }

    events.push(Ok(Event::flush(index)));
    events
}

fn map_message_delta(delta: &types::MessageDelta) -> Option<Event> {
    match delta.stop_reason.as_deref()? {
        "max_tokens" => Some(Event::Finished(FinishReason::MaxTokens)),
        _ => None,
    }
}

impl From<AnthropicError> for StreamError {
    fn from(error: AnthropicError) -> Self {
        use AnthropicError as E;

        match error {
            E::Network(error) => Self::from(error),
            E::StreamTransport(_) => StreamError::transient(error.to_string()).with_source(error),
            E::RateLimit { retry_after } => {
                Self::rate_limit(retry_after.map(Duration::from_secs)).with_source(error)
            }

            // Anthropic's API is notoriously unreliable, so we special-case a
            // few common errors that most of the times resolve themselves when
            // retried.
            //
            // See: <https://docs.claude.com/en/docs/build-with-claude/streaming#error-events>
            // See: <https://docs.claude.com/en/api/errors#http-errors>
            E::Api(ref api_error)
                if RETRYABLE_ANTHROPIC_ERROR_TYPES.contains(&api_error.error_type.as_str()) =>
            {
                let retry_after = api_error
                    .message
                    .as_deref()
                    .and_then(extract_retry_from_text)
                    .unwrap_or(Duration::from_secs(3));

                StreamError::transient(error.to_string())
                    .with_retry_after(retry_after)
                    .with_source(error)
            }

            // Detect billing/quota errors before falling through to generic.
            E::Api(ref api_error)
                if looks_like_quota_error(&api_error.error_type)
                    || api_error
                        .message
                        .as_deref()
                        .is_some_and(looks_like_quota_error) =>
            {
                StreamError::new(
                    StreamErrorKind::InsufficientQuota,
                    format!(
                        "Insufficient API quota. Check your plan and billing details \
                         at https://console.anthropic.com/settings/billing. ({error})"
                    ),
                )
                .with_source(error)
            }

            error => StreamError::other(error.to_string()).with_source(error),
        }
    }
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
