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
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{Name, ProviderId},
        parameters::ParametersConfig,
    },
    providers::llm::anthropic::AnthropicConfig,
};
use jp_conversation::{
    AssistantMessage, UserMessage,
    event::{ConversationEvent, EventKind},
    thread::{Document, Documents, Thread},
};
use serde_json::{Value, json};
use time::macros::date;
use tracing::{debug, info, trace, warn};

use super::{Event, EventStream, ModelDetails, Provider, ReasoningDetails, Reply};
use crate::{
    CompletionChunk, StreamEvent,
    error::{Error, Result},
    provider::ModelDeprecation,
    query::ChatQuery,
    stream::{accumulator::Accumulator, delta::Delta, event::StreamEndReason},
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Anthropic;

/// Anthropic limits the number of cache points to 4 per request. Returning an API error if the
/// request exceeds this limit.
///
/// We detect where we inject cache controls and make sure to not exceed this limit.
const MAX_CACHE_CONTROL_COUNT: usize = 4;

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
        let (mut after_id, mut models) = self
            .client
            .models()
            .list()
            .await
            .map(|list| (list.has_more.then_some(list.last_id).flatten(), list.data))?;

        while let Some(id) = &after_id {
            let (id, data) = self
                .client
                .get::<ListModelsResponse>(&format!("/v1/models?after_id={id}"))
                .await
                .map(|list| (list.has_more.then_some(list.last_id).flatten(), list.data))?;

            models.extend(data);
            after_id = id;
        }

        models
            .into_iter()
            .map(|v| map_model(v, &self.beta))
            .collect::<Result<_>>()
    }

    async fn chat_completion(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<Reply> {
        let request = create_request(model, parameters, query, false, &self.beta)?;

        debug!(stream = false, "Anthropic chat completion request.");
        trace!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            "Request payload."
        );

        self.client
            .messages()
            .create(request)
            .await
            .map_err(Into::into)
            .and_then(map_response)
            .map(|events| Reply {
                provider: PROVIDER,
                events,
            })
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<EventStream> {
        let client = self.client.clone();
        let chain_on_max_tokens = parameters.max_tokens.is_none() && self.chain_on_max_tokens;
        let request = create_request(model, parameters, query, true, &self.beta)?;

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
        let mut accumulator = Accumulator::new(200);
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
                        .contains(&e.error_type.as_str()) =>
                {
                    Error::RateLimit {
                        retry_after: Some(Duration::from_secs(3)),
                    }
                }

                _ => Error::from(e),
            });

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            for event in map_event(event?, &mut accumulator)? {
                if event.is_tool_call() {
                    tool_calls_requested = true;
                }

                if !tool_calls_requested && chain_on_max_tokens {
                    events.push(event.clone());
                }

                match event {
                    // If the assistant has reached the maximum number of
                    // tokens, and we are in a state in which we can request
                    // more tokens, we do so by sending a new request and
                    // chaining those events onto the previous ones, keeping the
                    // existing stream of events alive.
                    StreamEvent::EndOfStream(StreamEndReason::MaxTokens)
                        if !tool_calls_requested && chain_on_max_tokens =>
                    {
                        debug!("Max tokens reached, auto-requesting more tokens.");

                        for await event in chain(client.clone(), request.clone(), events) {
                            yield event?;
                        }
                        return;
                    }
                    eos @ StreamEvent::EndOfStream(_) => {
                        yield eos;
                        return;
                    }
                    event => yield event,
                }
            }
        }

        yield StreamEvent::EndOfStream(StreamEndReason::Completed);
    }))
}

/// Create a new `EventStream` by asking the assistant to continue from where it
/// left off.
fn chain(
    client: Client,
    mut request: types::CreateMessagesRequest,
    events: Vec<StreamEvent>,
) -> EventStream {
    let reply = AssistantMessage::from(Reply::from((PROVIDER, events)));
    debug_assert!(reply.tool_calls.is_empty());

    let mut should_merge = true;
    let previous_content = reply.content.clone().unwrap_or_default();
    let message = assistant_message_to_message(reply);

    request.messages.push(message);
    request.messages.push(types::Message {
        role: types::MessageRole::User,
        content: types::MessageContentList(vec![types::MessageContent::Text(
            "Please continue from where you left off.".into(),
        )]),
    });

    Box::pin(try_stream!({
        for await event in call(client, request, true) {
            match event? {
                StreamEvent::ChatChunk(chunk) => match chunk {
                    // When chaining new events, the reasoning content is
                    // irrelevant, as it will contain text such as "the user
                    // asked me to continue [...]".
                    CompletionChunk::Reasoning(_) => continue,
                    CompletionChunk::Content(mut text) => {
                        // Merge the new content with the previous content, if
                        // there is any overlap. Sometimes the assistant will
                        // start a chaining response with a small amount of
                        // content that was already seen in the previous
                        // response, and we want to avoid duplicating that.
                        if should_merge {
                            let merge_point = find_merge_point(&previous_content, &text, 500);
                            text.replace_range(..merge_point, "");
                        }

                        // After receiving the first content event, we can
                        // stop merging.
                        should_merge = false;

                        yield StreamEvent::ChatChunk(CompletionChunk::Content(text));
                    }
                },
                event => yield event,
            }
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
}

#[expect(clippy::too_many_lines)]
fn create_request(
    model: &ModelDetails,
    parameters: &ParametersConfig,
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
        history,
        message,
    } = thread;

    let mut cache_control_count = MAX_CACHE_CONTROL_COUNT;

    builder
        .model(model.id.name.clone())
        .messages(AnthropicMessages::build(history, message, &mut cache_control_count).0);

    let tools = convert_tools(tools, tool_call_strict_mode, &mut cache_control_count);

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

    let tool_choice_function = match tool_choice.clone() {
        ToolChoice::Function(name) => Some(name),
        _ => None,
    };

    // If there is only one tool, we can set the tool choice to "required",
    // since that gets us the same behavior, but avoids the issue of not
    // supporting reasoning when using the "function" tool choice.
    //
    // From testing, it seems that sending a single tool with the
    // "function" tool choice can result in incorrect API responses from
    // Anthropic. I (Jean) have an open support case with Anthropic to dig into
    // this finding more.
    if tools.len() == 1 && tool_choice_function.is_some() {
        tool_choice = ToolChoice::Required;
    }

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
    if reasoning_config.is_some()
        && let Some(tool) = tool_choice_function
    {
        info!(
            tool,
            "Anthropic API does not support reasoning when tool_choice forces tool use. Switching \
             to soft-force mode."
        );
        tool_choice = ToolChoice::Auto;
        system_content.push(types::SystemContent::Text(types::Text {
            text: format!(
                "IMPORTANT: You MUST use the function or tool named '{tool}' available to you. DO \
                 NOT QUESTION THIS DIRECTIVE. DO NOT PROMPT FOR MORE CONTEXT OR DETAILS. JUST RUN \
                 IT."
            ),
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
            Some(ReasoningDetails::Supported {
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
            _ => serde_json::Map::from_iter([(
                "edits".into(),
                json!([{"type": "clear_tool_uses_20250919"}]),
            )]),
        };

        builder.context_management(strategy);
    }

    builder.build().map_err(Into::into)
}

#[expect(clippy::match_same_arms, clippy::too_many_lines)]
fn map_model(model: types::Model, beta: &BetaFeatures) -> Result<ModelDetails> {
    let details = match model.id.as_str() {
        "claude-sonnet-4-5" | "claude-sonnet-4-5-20250929" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: if beta.context_1m() {
                Some(1_000_000)
            } else {
                Some(200_000)
            },
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::supported(1024, None)),
            knowledge_cutoff: Some(date!(2025 - 7 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec!["interleaved-thinking", "context-editing"],
        },
        "claude-opus-4-1" | "claude-opus-4-1-20250805" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(32_000),
            reasoning: Some(ReasoningDetails::supported(1024, None)),
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec!["interleaved-thinking", "context-editing"],
        },
        "claude-opus-4-0" | "claude-opus-4-20250514" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(32_000),
            reasoning: Some(ReasoningDetails::supported(1024, None)),
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
            reasoning: Some(ReasoningDetails::supported(1024, None)),
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec!["interleaved-thinking", "context-editing"],
        },
        "claude-3-7-sonnet-latest" | "claude-3-7-sonnet-20250219" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::supported(1024, None)),
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
        "claude-3-5-sonnet-latest"
        | "claude-3-5-sonnet-20241022"
        | "claude-3-5-sonnet-20240620" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some(model.display_name),
            context_window: Some(200_000),
            max_output_tokens: Some(8_192),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 4 - 1)),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: claude-sonnet-4-5-20250929",
                Some(date!(2025 - 10 - 22)),
            )),
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

fn map_response(response: types::CreateMessagesResponse) -> Result<Vec<Event>> {
    debug!("Received response from Anthropic API.");
    trace!(
        response = serde_json::to_string(&response).unwrap_or_default(),
        "Response payload."
    );

    response
        .content
        .into_iter()
        .filter_map(|item| {
            let metadata = match &item {
                types::MessageContent::Thinking(Thinking { signature, .. }) => {
                    signature.clone().map(|v| ("signature", v))
                }
                types::MessageContent::RedactedThinking { data } => {
                    Some(("redacted_thinking", data.clone()))
                }
                _ => None,
            };

            let v: Option<Result<Event>> = Delta::from(item).into();
            v.map(|v| (v, metadata))
        })
        .flat_map(|item| match item {
            (v, _) if v.is_err() => vec![v],
            (v, None) => vec![v],
            (v, Some((key, value))) => vec![v, Ok(Event::metadata(key, value))],
        })
        .collect::<Result<_>>()
}

fn map_event(
    event: types::MessagesStreamEvent,
    accumulator: &mut Accumulator,
) -> Result<Vec<StreamEvent>> {
    use types::{ContentBlockDelta::*, MessagesStreamEvent::*};

    trace!(
        event = serde_json::to_string(&event).unwrap_or_default(),
        "Received event from Anthropic API."
    );

    match event {
        MessageStart { message, .. } => message
            .content
            .into_iter()
            .map(|c| Delta::from(c).into_stream_events(accumulator))
            .try_fold(vec![], |mut acc, events| {
                acc.extend(events?);
                Ok(acc)
            }),
        ContentBlockStart { content_block, .. } => {
            Delta::from(content_block).into_stream_events(accumulator)
        }
        ContentBlockDelta { delta, .. } => match delta {
            TextDelta { text } => Delta::content(text).into_stream_events(accumulator),
            ThinkingDelta { thinking } => {
                Delta::reasoning(thinking).into_stream_events(accumulator)
            }
            InputJsonDelta { partial_json } => {
                Delta::tool_call("", "", partial_json).into_stream_events(accumulator)
            }

            // This is only used for thinking blocks, and we need to store this
            // signature to pass it back to the assistant in the message
            // history.
            //
            // See: <https://docs.anthropic.com/en/docs/build-with-claude/streaming#thinking-delta>
            SignatureDelta { signature } => Ok(vec![StreamEvent::metadata("signature", signature)]),
        },
        MessageDelta { delta, .. }
            if delta.stop_reason.as_ref().is_some_and(|v| v == "tool_use") =>
        {
            Delta::tool_call_finished().into_stream_events(accumulator)
        }
        ContentBlockStop { .. } => accumulator.drain(),
        MessageDelta { delta, .. }
            if delta
                .stop_reason
                .as_ref()
                .is_some_and(|v| v == "max_tokens") =>
        {
            Ok(vec![StreamEvent::EndOfStream(StreamEndReason::MaxTokens)])
        }
        _ => Ok(vec![]),
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
    _strict: bool,
    cache_controls: &mut usize,
) -> Vec<types::Tool> {
    let mut tools: Vec<_> = tools
        .into_iter()
        .map(|tool| {
            types::Tool::Custom(types::CustomTool {
                name: tool.name,
                description: tool.description,
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
    fn build(
        history: Vec<ConversationEvent>,
        message: UserMessage,
        cache_controls: &mut usize,
    ) -> Self {
        let mut items = vec![];

        // Historical messages.
        let mut history = history
            .into_iter()
            .filter_map(event_to_message)
            .collect::<Vec<_>>();

        // Make sure to add cache control to the last history message.
        if *cache_controls > 0
            && let Some(message) = history.last_mut().and_then(|m| m.content.0.last_mut())
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

        items.extend(history);

        // User query
        match message {
            UserMessage::Query { query } => {
                items.push(types::Message {
                    role: types::MessageRole::User,
                    content: types::MessageContentList(vec![types::MessageContent::Text(
                        query.into(),
                    )]),
                });
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| types::Message {
                    role: types::MessageRole::User,
                    content: types::MessageContentList(vec![types::MessageContent::ToolResult(
                        types::ToolResult {
                            tool_use_id: result.id,
                            content: Some(result.content),
                            is_error: result.error,
                            cache_control: None,
                        },
                    )]),
                }));
            }
        }

        Self(items)
    }
}

fn event_to_message(event: ConversationEvent) -> Option<types::Message> {
    match event.kind {
        EventKind::UserMessage(user) => Some(user_message_to_message(user)),
        EventKind::AssistantMessage(assistant) => Some(assistant_message_to_message(assistant)),
        EventKind::ConfigDelta(_) => None,
    }
}

fn user_message_to_message(user: UserMessage) -> types::Message {
    let list = match user {
        UserMessage::Query { query } => vec![types::MessageContent::Text(query.into())],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| {
                types::MessageContent::ToolResult(types::ToolResult {
                    tool_use_id: result.id,
                    content: Some(result.content),
                    is_error: result.error,
                    cache_control: None,
                })
            })
            .collect(),
    };

    types::Message {
        role: types::MessageRole::User,
        content: types::MessageContentList(list),
    }
}

fn assistant_message_to_message(assistant: AssistantMessage) -> types::Message {
    let AssistantMessage {
        provider,
        reasoning,
        content,
        tool_calls,
        metadata,
    } = assistant;

    let mut list = vec![];
    if let Some(data) = metadata
        .get("redacted_thinking")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        list.push(types::MessageContent::RedactedThinking { data });
    } else if let Some(thinking) = reasoning {
        if provider == PROVIDER {
            list.push(types::MessageContent::Thinking(Thinking {
                thinking,
                signature: metadata
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }));
        } else {
            list.push(types::MessageContent::Text(
                format!("<think>\n{thinking}\n</think>\n\n").into(),
            ));
        }
    }

    if let Some(text) = content {
        list.push(types::MessageContent::Text(text.into()));
    }

    for tool_call in tool_calls {
        list.push(types::MessageContent::ToolUse(types::ToolUse {
            id: tool_call.id,
            input: Value::Object(tool_call.arguments),
            name: tool_call.name,
            cache_control: None,
        }));
    }

    types::Message {
        role: types::MessageRole::Assistant,
        content: types::MessageContentList(list),
    }
}

impl From<types::MessageContent> for Delta {
    fn from(item: types::MessageContent) -> Self {
        match item {
            types::MessageContent::Text(text) => Delta::content(text.text),
            types::MessageContent::Thinking(thinking) => Delta::reasoning(thinking.thinking),
            types::MessageContent::RedactedThinking { .. } => Delta::reasoning(String::new()),
            types::MessageContent::ToolUse(tool_use) => {
                Delta::tool_call(tool_use.id, tool_use.name, match &tool_use.input {
                    Value::Object(map) if !map.is_empty() => tool_use.input.to_string(),
                    _ => String::new(),
                })
            }
            types::MessageContent::ToolResult(_) => {
                debug_assert!(false, "Unexpected message content: {item:?}");
                Delta::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use indexmap::IndexMap;
    use jp_config::{
        conversation::tool::{OneOrManyTypes, ToolParameterConfig, ToolParameterItemsConfig},
        model::parameters::{CustomReasoningConfig, ReasoningEffort},
        providers::llm::LlmProviderConfig,
    };
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr() -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new("https://api.anthropic.com", fixtures)
    }

    #[test(tokio::test)]
    async fn test_anthropic_model_details() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().anthropic;
        let name: Name = "claude-3-5-haiku-latest".parse().unwrap();

        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = url;
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Anthropic::try_from(&config)
                    .unwrap()
                    .model_details(&name)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().anthropic;

        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = url;
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Anthropic::try_from(&config).unwrap().models().await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_chat_completion() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let mut config = LlmProviderConfig::default().anthropic;
        let model_id = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
        let model = ModelDetails::empty(model_id);
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = url;
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model, &ParametersConfig::default(), query)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_tool_call() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().anthropic;
        let model_id = "anthropic/claude-3-7-sonnet-latest".parse().unwrap();
        let model = ModelDetails::empty(model_id);
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            tool_choice: ToolChoice::Function("run_me".to_owned()),
            tools: vec![ToolDefinition {
                name: "run_me".to_owned(),
                description: None,
                parameters: IndexMap::from_iter([
                    ("foo".to_owned(), ToolParameterConfig {
                        kind: OneOrManyTypes::One("string".into()),
                        default: Some("foo".into()),
                        description: None,
                        required: false,
                        enumeration: vec![],
                        items: None,
                    }),
                    ("bar".to_owned(), ToolParameterConfig {
                        kind: OneOrManyTypes::Many(vec!["string".into(), "array".into()]),
                        default: None,
                        description: None,
                        required: true,
                        enumeration: vec!["foo".into(), vec!["foo", "bar"].into()],
                        items: Some(ToolParameterItemsConfig {
                            kind: "string".to_owned(),
                        }),
                    }),
                ]),
            }],
            ..Default::default()
        };

        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = url;
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model, &ParametersConfig::default(), query)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_redacted_thinking()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().anthropic;
        let model_id = "anthropic/claude-3-7-sonnet-latest".parse().unwrap();
        let model = ModelDetails::empty(model_id);
        let query = ChatQuery {
            thread: Thread {
                // See: <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking#thinking-redaction>
                message: "ANTHROPIC_MAGIC_STRING_TRIGGER_REDACTED_THINKING_46C9A13E193C177646C7398A98432ECCCE4C1253D5E2D82641AC0E52CC2876CB".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = url;
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                let parameters = ParametersConfig {
                    reasoning: Some(
                        CustomReasoningConfig {
                            effort: ReasoningEffort::Medium,
                            exclude: false,
                        }
                        .into(),
                    ),
                    ..Default::default()
                };

                let events = Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model, &parameters, query.clone())
                    .await
                    .unwrap()
                    .into_inner();

                assert!(events.iter().any(
                    |event| matches!(event, Event::Metadata(k, _) if k == "redacted_thinking")
                ));

                events
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_request_chaining() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let mut config = LlmProviderConfig::default().anthropic;
        let model_id = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
        let mut model = ModelDetails::empty(model_id);
        model.max_output_tokens = Some(1024);

        let query = ChatQuery {
            thread: Thread {
                message: "Give me a 2000 word explainer about Kirigami-inspired parachutes".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = url;
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion_stream(&model, &ParametersConfig::default(), query)
                    .await
                    .unwrap()
                    .collect::<Vec<_>>()
                    .await
            },
        )
        .await
    }

    #[test]
    fn test_create_request() {
        let model_id = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
        let model = ModelDetails::empty(model_id);
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let parameters = ParametersConfig {
            top_p: Some(1.0),
            top_k: Some(40),
            reasoning: Some(
                CustomReasoningConfig {
                    effort: ReasoningEffort::Medium,
                    exclude: false,
                }
                .into(),
            ),
            ..Default::default()
        };

        let request = create_request(&model, &parameters, query, false, &BetaFeatures::default());

        insta::assert_debug_snapshot!(request);
    }

    #[test]
    fn test_find_merge_point_edge_cases() {
        struct TestCase {
            left: &'static str,
            right: &'static str,
            expected: &'static str,
            max_search: usize,
        }

        let cases = IndexMap::from([
            ("no overlap", TestCase {
                left: "Hello",
                right: " world",
                expected: "Hello world",
                max_search: 500,
            }),
            ("single word overlap", TestCase {
                left: "The quick brown",
                right: "brown fox",
                expected: "The quick brown fox",
                max_search: 500,
            }),
            ("minimal overlap (5 chars)", TestCase {
                expected: "abcdefghij",
                left: "abcdefgh",
                right: "defghij",
                max_search: 500,
            }),
            (
                "below minimum overlap (4 chars) - should not merge",
                TestCase {
                    left: "abcd",
                    right: "abcd",
                    expected: "abcdabcd",
                    max_search: 500,
                },
            ),
            ("complete overlap", TestCase {
                left: "Hello world",
                right: "world",
                expected: "Hello world",
                max_search: 500,
            }),
            ("overlap with punctuation", TestCase {
                left: "Hello, how are",
                right: "how are you?",
                expected: "Hello, how are you?",
                max_search: 500,
            }),
            ("overlap with whitespace", TestCase {
                left: "Hello     ",
                right: "     world",
                expected: "Hello     world",
                max_search: 500,
            }),
            ("unicode overlap", TestCase {
                left: "Hello 世界",
                right: "世界 friend",
                expected: "Hello 世界 friend",
                max_search: 500,
            }),
            ("long overlap", TestCase {
                left: "The quick brown fox jumps",
                right: "fox jumps over the lazy dog",
                expected: "The quick brown fox jumpsfox jumps over the lazy dog",
                max_search: 8,
            }),
            ("empty right", TestCase {
                left: "Hello",
                right: "",
                expected: "Hello",
                max_search: 500,
            }),
        ]);

        for (
            name,
            TestCase {
                left,
                right,
                expected,
                max_search,
            },
        ) in cases
        {
            let pos = find_merge_point(left, right, max_search);
            let result = format!("{left}{}", &right[pos..]);
            assert_eq!(result, expected, "Failed test case: {name}");
        }
    }
}
