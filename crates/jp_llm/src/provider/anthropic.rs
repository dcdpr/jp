use std::{env, time::Duration};

use async_anthropic::{
    Client,
    errors::AnthropicError,
    messages::DEFAULT_MAX_TOKENS,
    types::{
        self, ListModelsResponse, System, Thinking, ToolBash, ToolCodeExecution, ToolComputerUse,
        ToolTextEditor, ToolWebSearch,
    },
};
use async_stream::try_stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use indexmap::IndexMap;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::id::{Name, ProviderId},
    providers::llm::anthropic::AnthropicConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, ConversationEvent, EventKind},
    thread::{Document, Documents, Thread},
};
use serde_json::{Map, Value, json};
use time::macros::date;
use tracing::{debug, info, trace, warn};

use super::Provider;
use crate::{
    error::{Error, Result},
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
        let chain_on_max_tokens = query
            .thread
            .events
            .config()?
            .assistant
            .model
            .parameters
            .max_tokens
            .is_none()
            && self.chain_on_max_tokens;

        let request = create_request(model, query, true, &self.beta)?;

        debug!(stream = true, "Anthropic chat completion stream request.");
        trace!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            "Request payload."
        );

        Ok(call(client, request, chain_on_max_tokens))
    }
}

/// Create a request to the assistant to generate a response, and return a
/// stream of [`StreamEvent`]s.
///
/// If `chain_on_max_tokens` is `true`, a new request is created when the last
/// one ends with a [`StreamEndReason::MaxTokens`] event, allowing the assistant
/// to continue from where it left off and the caller to receive the full
/// response as a single stream of events.
fn call(
    client: Client,
    request: types::CreateMessagesRequest,
    chain_on_max_tokens: bool,
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
            .map_err(|e| match e {
                AnthropicError::RateLimit { retry_after } => Error::RateLimit {
                    retry_after: retry_after.map(Duration::from_secs),
                },

                // Anthropic's API is notoriously unreliable, so we
                // special-case a few common errors that most of the times
                // resolve themselves when retried.
                //
                // See: <https://docs.claude.com/en/docs/build-with-claude/streaming#error-events>
                // See: <https://docs.claude.com/en/api/errors#http-errors>
                AnthropicError::StreamError(e)
                    if ["rate_limit_error", "overloaded_error", "api_error"]
                        .contains(&e.error_type.as_str())
                        || e.error.as_ref().is_some_and(|v| {
                            v.get("message").and_then(Value::as_str) == Some("Overloaded")
                        }) =>
                {
                    Error::RateLimit {
                        retry_after: Some(Duration::from_secs(3)),
                    }
                }

                _ => Error::from(e),
            });

        tokio::pin!(stream);
        while let Some(event) = stream.next().await.transpose()? {
            if let Some(event) = map_event(event, &mut tool_call_aggregator)? {
                match event {
                    // If the assistant has reached the maximum number of
                    // tokens, and we are in a state in which we can request
                    // more tokens, we do so by sending a new request and
                    // chaining those events onto the previous ones, keeping the
                    // existing stream of events alive.
                    //
                    // TODO: generalize this for any provider.
                    Event::Finished(FinishReason::MaxTokens)
                        if !tool_calls_requested && chain_on_max_tokens =>
                    {
                        debug!("Max tokens reached, auto-requesting more tokens.");

                        for await event in chain(client.clone(), request.clone(), events) {
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
                    flush @ Event::Flush { .. } => yield flush,
                }
            }
        }

        yield Event::Finished(FinishReason::Completed);
    }))
}

/// Create a new `EventStream` by asking the assistant to continue from where it
/// left off.
fn chain(
    client: Client,
    mut request: types::CreateMessagesRequest,
    events: Vec<ConversationEvent>,
) -> EventStream {
    debug_assert!(!events.iter().any(ConversationEvent::is_tool_call_request));

    let mut should_merge = true;
    let previous_content = events
        .last()
        .and_then(|e| e.as_chat_response().map(ChatResponse::content))
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
             continue the conversation, just do it. DO NOT produce any tokens that overlap with \
             the already generated tokens, start exactly from the token that was generated last."
                .into(),
        )]),
    });

    Box::pin(try_stream!({
        for await event in call(client, request, true) {
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
            if let Some(content) = event
                .as_conversation_event_mut()
                .and_then(ConversationEvent::as_chat_response_mut)
                .map(ChatResponse::content_mut)
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
) -> Result<types::CreateMessagesRequest> {
    let ChatQuery {
        thread,
        tools,
        mut tool_choice,
        tool_call_strict_mode,
    } = query;

    let mut builder = types::CreateMessagesRequestBuilder::default();

    builder.stream(stream);

    let Thread {
        system_prompt,
        instructions,
        attachments,
        events,
    } = thread;

    let mut cache_control_count = MAX_CACHE_CONTROL_COUNT;
    let config = events.config()?;

    builder
        .model(model.id.name.clone())
        .messages(AnthropicMessages::build(events, &mut cache_control_count).0);

    let tools = convert_tools(
        tools,
        tool_call_strict_mode
            && model.features.contains(&"structured-outputs")
            && beta.structured_outputs(),
        &mut cache_control_count,
    );

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

    if !instructions.is_empty() {
        let text = instructions
            .into_iter()
            .map(|instruction| instruction.try_to_xml().map_err(Into::into))
            .collect::<Result<Vec<_>>>()?
            .join("\n\n");

        system_content.push(types::SystemContent::Text(types::Text {
            text,
            cache_control: (cache_control_count > 0).then_some({
                cache_control_count = cache_control_count.saturating_sub(1);
                types::CacheControl::default()
            }),
        }));
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

    if let Some(config) = reasoning_config {
        let (min_budget, mut max_budget) = match model.reasoning {
            Some(ReasoningDetails::Budgetted {
                min_tokens,
                max_tokens,
            }) => (min_tokens, max_tokens.unwrap_or(u32::MAX)),
            _ => (0, u32::MAX),
        };

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

        builder.thinking(types::ExtendedThinking {
            kind: "enabled".to_string(),
            budget_tokens: config
                .effort
                .to_tokens(max_tokens)
                .max(min_budget)
                .min(max_budget),
        });
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

    builder.build().map_err(Into::into)
}

#[expect(clippy::too_many_lines)]
fn map_model(model: types::Model, beta: &BetaFeatures) -> Result<ModelDetails> {
    let details = match model.id.as_str() {
        "claude-opus-4-5" | "claude-opus-4-5-20251101" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: Some(date!(2025 - 7 - 1)),
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
            knowledge_cutoff: Some(date!(2025 - 7 - 1)),
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
            knowledge_cutoff: Some(date!(2025 - 7 - 1)),
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
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
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
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
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
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec!["interleaved-thinking", "context-editing"],
        },
        "claude-3-7-sonnet-latest" | "claude-3-7-sonnet-20250219" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: Some(date!(2024 - 11 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "claude-3-5-haiku-latest" | "claude-3-5-haiku-20241022" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(8_192),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 7 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "claude-3-opus-latest" | "claude-3-opus-20240229" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2023 - 8 - 1)),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: claude-opus-4-1-20250805",
                Some(date!(2026 - 1 - 5)),
            )),
            features: vec![],
        },
        "claude-3-haiku-20240307" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 8 - 1)),
            deprecated: Some(ModelDeprecation::Active),
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
) -> Result<Option<Event>> {
    use types::MessagesStreamEvent::*;

    trace!(
        event = serde_json::to_string(&event).unwrap_or_default(),
        "Received event from Anthropic API."
    );

    match event {
        ContentBlockStart {
            content_block,
            index,
        } => Ok(map_content_start(content_block, index, agg)),
        ContentBlockDelta { delta, index } => Ok(map_content_delta(delta, index, agg)),
        ContentBlockStop { index } => map_content_stop(index, agg),
        MessageDelta { delta, .. } => Ok(map_message_delta(delta)),
        _ => Ok(None),
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
        EventKind::ChatRequest(request) => Some((
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
                    thinking: response.into_content(),
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
            } else if response.is_reasoning() {
                // Reasoning from other providers - wrap in XML tags
                types::MessageContent::Text(
                    format!("<think>\n{}\n</think>\n\n", response.content()).into(),
                )
            } else {
                types::MessageContent::Text(response.into_content().into())
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
        EventKind::InquiryRequest(_) | EventKind::InquiryResponse(_) => None,
    }
}

fn map_content_start(
    item: types::MessageContent,
    index: usize,
    agg: &mut ToolCallRequestAggregator,
) -> Option<Event> {
    use types::MessageContent::*;

    let mut metadata = IndexMap::new();
    let kind: EventKind = match item {
        ToolUse(types::ToolUse { id, name, .. }) => {
            *agg = ToolCallRequestAggregator::new();
            agg.add_chunk(index, Some(id), Some(name), None);
            return None;
        }
        Text(text) => ChatResponse::message(text.text).into(),
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
) -> Option<Event> {
    let mut metadata = IndexMap::new();
    let kind: EventKind = match delta {
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

fn map_content_stop(index: usize, agg: &mut ToolCallRequestAggregator) -> Result<Option<Event>> {
    // Check if we're buffering a tool call request
    match agg.finalize(index) {
        Ok(tool_call) => {
            return Ok(Some(Event::Part {
                event: ConversationEvent::now(tool_call),
                index,
            }));
        }
        Err(AggregationError::UnknownIndex) => {}
        Err(error) => return Err(error.into()),
    }

    Ok(Some(Event::flush(index)))
}

fn map_message_delta(delta: types::MessageDelta) -> Option<Event> {
    match delta.stop_reason?.as_str() {
        "max_tokens" => Some(Event::Finished(FinishReason::MaxTokens)),
        _ => None,
    }
}

// #[cfg(test)]
// mod tests {
//     use indexmap::IndexMap;
//     use jp_config::{
//         conversation::tool::{OneOrManyTypes, ToolParameterConfig, ToolParameterItemsConfig},
//         providers::llm::LlmProviderConfig,
//     };
//     use jp_conversation::event::ChatRequest;
//     use jp_test::{Result, fn_name, mock::Vcr};
//     use test_log::test;
//
//     use super::*;
//     use crate::test::Req;
//
//     const MAGIC_STRING: &str = "ANTHROPIC_MAGIC_STRING_TRIGGER_REDACTED_THINKING_46C9A13E193C177646C7398A98432ECCCE4C1253D5E2D82641AC0E52CC2876CB";
//     const TEST_MODEL: &str = "anthropic/claude-haiku-4-5";
//
//     fn vcr() -> Vcr {
//         Vcr::new("https://api.anthropic.com", env!("CARGO_MANIFEST_DIR"))
//     }
//
//     async fn run_chat_completion(
//         test_name: impl AsRef<str>,
//         requests: impl IntoIterator<Item = Req>,
//         config: Option<LlmProviderConfig>,
//     ) -> std::result::Result<(), Box<dyn std::error::Error>> {
//         crate::test::run_chat_completion(
//             test_name,
//             env!("CARGO_MANIFEST_DIR"),
//             ProviderId::Anthropic,
//             config.unwrap_or_default(),
//             requests.into_iter().collect(),
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_anthropic_model_details() -> Result {
//         let mut config = LlmProviderConfig::default().anthropic;
//         let name: Name = "claude-3-5-haiku-latest".parse().unwrap();
//
//         let vcr = vcr();
//         vcr.cassette(
//             fn_name!(),
//             |rule| {
//                 rule.filter(|when| {
//                     when.any_request();
//                 });
//             },
//             |recording, url| async move {
//                 config.base_url = url;
//                 if !recording {
//                     // dummy api key value when replaying a cassette
//                     config.api_key_env = "USER".to_owned();
//                 }
//
//                 Anthropic::try_from(&config)
//                     .unwrap()
//                     .model_details(&name)
//                     .await
//             },
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_anthropic_models() -> Result {
//         let mut config = LlmProviderConfig::default().anthropic;
//
//         let vcr = vcr();
//         vcr.cassette(
//             fn_name!(),
//             |rule| {
//                 rule.filter(|when| {
//                     when.any_request();
//                 });
//             },
//             |recording, url| async move {
//                 config.base_url = url;
//                 if !recording {
//                     // dummy api key value when replaying a cassette
//                     config.api_key_env = "USER".to_owned();
//                 }
//
//                 Anthropic::try_from(&config).unwrap().models().await
//             },
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_anthropic_no_stream() -> Result {
//         let request = Req::new()
//             .stream(false)
//             .model(TEST_MODEL)
//             .enable_reasoning()
//             .event(ChatRequest::from("Test message"));
//
//         run_chat_completion(fn_name!(), Some(request), None).await
//     }
//
//     #[test(tokio::test)]
//     async fn test_anthropic_stream() -> Result {
//         let request = Req::new()
//             .stream(true)
//             .model(TEST_MODEL)
//             .enable_reasoning()
//             .event(ChatRequest::from("Test message"));
//
//         run_chat_completion(fn_name!(), Some(request), None).await
//     }
//
//     #[test(tokio::test)]
//     async fn test_anthropic_multi_turn() -> Result {
//         let base = Req::new().stream(true).model(TEST_MODEL).enable_reasoning();
//
//         let requests = vec![
//             base.clone().chat_request("Test message"),
//             base.clone().chat_request("Repeat my previous message"),
//         ];
//
//         run_chat_completion(fn_name!(), requests, None).await
//     }
//
//     #[test(tokio::test)]
//     async fn test_anthropic_tool_call() -> Result {
//         let base = Req::new()
//             .model(TEST_MODEL)
//             .enable_reasoning()
//             .event(ChatRequest::from("Test message"))
//             .tool_choice_fn("run_me")
//             .tool_call_strict_mode(false)
//             .tool("run_me", vec![
//                 ("foo", ToolParameterConfig {
//                     kind: OneOrManyTypes::One("string".into()),
//                     default: Some("foo".into()),
//                     description: None,
//                     required: false,
//                     enumeration: vec![],
//                     items: None,
//                 }),
//                 ("bar", ToolParameterConfig {
//                     kind: OneOrManyTypes::Many(vec!["string".into(), "array".into()]),
//                     default: None,
//                     description: None,
//                     required: true,
//                     enumeration: vec!["foo".into(), vec!["foo", "bar"].into()],
//                     items: Some(ToolParameterItemsConfig {
//                         kind: "string".to_owned(),
//                     }),
//                 }),
//             ]);
//
//         let cases = vec![
//             ("streaming", base.clone().stream(true)),
//             ("no_streaming", base.clone()),
//             ("strict", base.clone().tool_call_strict_mode(true)),
//             ("required", base.clone().tool_choice(ToolChoice::Required)),
//             ("auto", base.clone().tool_choice(ToolChoice::Auto)),
//             ("no_reasoning", base.clone().reasoning(None)),
//         ];
//
//         for (name, request) in cases {
//             run_chat_completion(format!("{}_{name}", fn_name!()), Some(request), None).await?;
//         }
//
//         Ok(())
//     }
//
//     #[test(tokio::test)]
//     async fn test_anthropic_redacted_thinking() -> Result {
//         let base = Req::new()
//             .stream(true)
//             .model(TEST_MODEL)
//             .enable_reasoning()
//             .assert(|events| {
//                 assert!(events.iter().any(
//                         |event| matches!(event, Event::Part { event, .. } if event.metadata.contains_key(REDACTED_THINKING_KEY))
//                     ));
//             });
//
//         let requests = vec![
//             base.clone().chat_request(MAGIC_STRING),
//             base.clone()
//                 .chat_request("Do you have access to your redacted thinking content?"),
//         ];
//
//         run_chat_completion(fn_name!(), requests, None).await
//     }
//
//     #[test(tokio::test)]
//     async fn test_anthropic_request_chaining() -> Result {
//         let request = Req::new()
//             .stream(true)
//             .model_details(ModelDetails {
//                 id: TEST_MODEL.parse().unwrap(),
//                 max_output_tokens: Some(1024),
//                 context_window: None,
//                 display_name: None,
//                 reasoning: None,
//                 knowledge_cutoff: None,
//                 deprecated: None,
//                 features: vec![],
//             })
//             .enable_reasoning()
//             .chat_request("Give me a 2000 word explainer about Kirigami-inspired parachutes");
//
//         run_chat_completion(fn_name!(), Some(request), None).await
//     }
//
//     // #[test]
//     // fn test_create_request() {
//     //     let model_id = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
//     //     let model = ModelDetails::empty(model_id);
//     //     let query = ChatQuery {
//     //         thread: Thread {
//     //             events: ConversationStream::default().with_chat_request("Test message"),
//     //             ..Default::default()
//     //         },
//     //         ..Default::default()
//     //     };
//     //
//     //     // let parameters = ParametersConfig {
//     //     //     top_p: Some(1.0),
//     //     //     top_k: Some(40),
//     //     //     reasoning: Some(
//     //     //         CustomReasoningConfig {
//     //     //             effort: ReasoningEffort::Medium,
//     //     //             exclude: false,
//     //     //         }
//     //     //         .into(),
//     //     //     ),
//     //     //     ..Default::default()
//     //     // };
//     //
//     //     let request = create_request(&model, query, false, &BetaFeatures::default());
//     //
//     //     insta::assert_debug_snapshot!(request);
//     // }
//
//     #[test]
//     fn test_find_merge_point_edge_cases() {
//         struct TestCase {
//             left: &'static str,
//             right: &'static str,
//             expected: &'static str,
//             max_search: usize,
//         }
//
//         let cases = IndexMap::from([
//             ("no overlap", TestCase {
//                 left: "Hello",
//                 right: " world",
//                 expected: "Hello world",
//                 max_search: 500,
//             }),
//             ("single word overlap", TestCase {
//                 left: "The quick brown",
//                 right: "brown fox",
//                 expected: "The quick brown fox",
//                 max_search: 500,
//             }),
//             ("minimal overlap (5 chars)", TestCase {
//                 expected: "abcdefghij",
//                 left: "abcdefgh",
//                 right: "defghij",
//                 max_search: 500,
//             }),
//             (
//                 "below minimum overlap (4 chars) - should not merge",
//                 TestCase {
//                     left: "abcd",
//                     right: "abcd",
//                     expected: "abcdabcd",
//                     max_search: 500,
//                 },
//             ),
//             ("complete overlap", TestCase {
//                 left: "Hello world",
//                 right: "world",
//                 expected: "Hello world",
//                 max_search: 500,
//             }),
//             ("overlap with punctuation", TestCase {
//                 left: "Hello, how are",
//                 right: "how are you?",
//                 expected: "Hello, how are you?",
//                 max_search: 500,
//             }),
//             ("overlap with whitespace", TestCase {
//                 left: "Hello     ",
//                 right: "     world",
//                 expected: "Hello     world",
//                 max_search: 500,
//             }),
//             ("unicode overlap", TestCase {
//                 left: "Hello 世界",
//                 right: "世界 friend",
//                 expected: "Hello 世界 friend",
//                 max_search: 500,
//             }),
//             ("long overlap", TestCase {
//                 left: "The quick brown fox jumps",
//                 right: "fox jumps over the lazy dog",
//                 expected: "The quick brown fox jumpsfox jumps over the lazy dog",
//                 max_search: 8,
//             }),
//             ("empty right", TestCase {
//                 left: "Hello",
//                 right: "",
//                 expected: "Hello",
//                 max_search: 500,
//             }),
//         ]);
//
//         for (
//             name,
//             TestCase {
//                 left,
//                 right,
//                 expected,
//                 max_search,
//             },
//         ) in cases
//         {
//             let pos = find_merge_point(left, right, max_search);
//             let result = format!("{left}{}", &right[pos..]);
//             assert_eq!(result, expected, "Failed test case: {name}");
//         }
//     }
// }
