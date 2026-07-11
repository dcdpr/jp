use std::{env, time::Duration};

use async_trait::async_trait;
use base64::Engine as _;
use chrono::NaiveDate;
use futures::{StreamExt as _, TryStreamExt as _, stream};
use indexmap::{IndexMap, IndexSet};
use jp_attachment::AttachmentContent;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    conversation::tool::{OneOrManyTypes, ToolParameterConfig},
    model::{
        id::{Name, ProviderId},
        parameters::{CustomReasoningConfig, ReasoningEffort},
    },
    providers::llm::openai::OpenaiConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, ConversationEvent, EventKind, ToolCallResponse},
    thread::text_attachments_to_xml,
};
use openai_responses::{
    Client, CreateError, StreamError as OpenaiStreamError,
    types::{self, Include, Request, SummaryConfig},
};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Map, Value};
use tracing::{debug, trace, warn};

use super::{EventStream, ModelDetails, Provider};
use crate::{
    error::{
        Error, Result, StreamError, StreamErrorKind, extract_retry_from_text,
        looks_like_quota_error,
    },
    event::{Event, FinishReason},
    model::{ModelDeprecation, ReasoningDetails},
    provider::trace_to_tmpfile,
    query::ChatQuery,
    stream::with_tool_call_keepalive,
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Openai;

pub(crate) const ITEM_ID_KEY: &str = "openai_item_id";
pub(crate) const ENCRYPTED_CONTENT_KEY: &str = "openai_encrypted_content";
pub(crate) const PHASE_KEY: &str = "openai_phase";

/// Feature flag: temperature and `top_p` are only supported when reasoning
/// effort is `none`.
/// GPT-5 family models have this constraint.
const TEMP_REQUIRES_NO_REASONING: &str = "temp_requires_no_reasoning";

/// Feature flag: the model only supports non-streaming Responses API requests.
const STREAMING_UNSUPPORTED: &str = "streaming_unsupported";

/// Feature flag: the model accepts `reasoning.mode: "pro"` in the Responses
/// API.
/// Models without this flag reject the field, so pro mode is skipped (with a
/// warning) when configured.
const REASONING_PRO_MODE: &str = "reasoning_pro_mode";

/// Feature flag: the model supports persisted reasoning via
/// `reasoning.context`.
/// When set, requests ask for `all_turns` so the model renders the replayed
/// encrypted reasoning items from earlier turns into the next sample.
const PERSISTED_REASONING: &str = "persisted_reasoning";

/// Feature flag: the model accepts explicit prompt-cache fields
/// (`prompt_cache_options`, `prompt_cache_breakpoint`).
/// Models without this flag reject the fields with a 400, so they are only sent
/// when the flag is present.
const EXPLICIT_PROMPT_CACHING: &str = "explicit_prompt_caching";

/// How often to inject a synthetic keep-alive while a tool call is streaming.
///
/// OpenAI emits the `function_call_arguments` deltas for a large tool call as a
/// burst that can follow a silent gap, which the idle timeout would otherwise
/// treat as a dead connection.
/// This interval stays comfortably below the enforced minimum
/// `stream_idle_timeout_secs` (10s), so the heartbeat always lands before the
/// idle window elapses.
const TOOL_CALL_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Openai {
    reqwest_client: reqwest::Client,
    client: Client,
    base_url: String,
}

#[async_trait]
impl Provider for Openai {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails> {
        self.reqwest_client
            .get(format!("{}/v1/models/{}", self.base_url, name))
            .send()
            .await?
            .error_for_status()?
            .json::<ModelResponse>()
            .await
            .map_err(Into::into)
            .and_then(map_model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>> {
        self.reqwest_client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<ModelListResponse>()
            .await?
            .data
            .into_iter()
            .map(map_model)
            .collect::<Result<_>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream> {
        let (request, is_structured, reasoning_enabled) = create_request(model, query)?;

        if model.features.contains(&STREAMING_UNSUPPORTED) {
            let response = self.client.create(request).await??;
            let events = map_non_streaming_response(response, is_structured, reasoning_enabled)?;

            return Ok(stream::iter(events.into_iter().map(Ok::<_, StreamError>)).boxed());
        }

        let raw_stream = self
            .client
            .stream(request)
            .filter_map(skip_unknown_events)
            .or_else(map_error)
            .map_ok(move |v| stream::iter(map_event(v, is_structured, reasoning_enabled)))
            .try_flatten()
            .boxed();

        Ok(with_tool_call_keepalive(
            raw_stream,
            TOOL_CALL_KEEPALIVE_INTERVAL,
        ))
    }
}

fn map_non_streaming_response(
    response: types::response::Response,
    is_structured: bool,
    reasoning_enabled: bool,
) -> Result<Vec<Event>> {
    let incomplete_reason = response.incomplete_details.map(|details| details.reason);
    let mut events = response
        .output
        .into_iter()
        .enumerate()
        .flat_map(|(index, item)| synthesize_non_streaming_output_item_events(index, item))
        .flat_map(|event| map_event(event, is_structured, reasoning_enabled))
        .collect::<std::result::Result<Vec<_>, StreamError>>()?;

    events.push(map_non_streaming_finish_reason(
        response.status,
        incomplete_reason,
    )?);

    Ok(events)
}

fn synthesize_non_streaming_output_item_events(
    index: usize,
    item: types::OutputItem,
) -> Vec<types::Event> {
    let output_index = index as u64;

    match item {
        types::OutputItem::Message(message) => {
            let mut events = vec![types::Event::OutputItemAdded {
                item: types::OutputItem::Message(message.clone()),
                output_index,
            }];

            for (content_index, content) in message.content.iter().enumerate() {
                match content {
                    types::OutputContent::Text { text, .. } => {
                        events.push(types::Event::OutputTextDelta {
                            content_index: content_index as u64,
                            delta: text.clone(),
                            item_id: message.id.clone(),
                            output_index,
                        });
                    }
                    types::OutputContent::Refusal { refusal } => {
                        events.push(types::Event::RefusalDelta {
                            content_index: content_index as u64,
                            delta: refusal.clone(),
                            item_id: message.id.clone(),
                            output_index,
                        });
                    }
                }
            }

            events.push(types::Event::OutputItemDone {
                item: types::OutputItem::Message(message),
                output_index,
            });
            events
        }
        types::OutputItem::Reasoning(reasoning) => {
            let mut events = vec![types::Event::OutputItemAdded {
                item: types::OutputItem::Reasoning(reasoning.clone()),
                output_index,
            }];

            for (summary_index, summary) in reasoning.summary.iter().enumerate() {
                let types::ReasoningSummary::Text { text } = summary;
                events.push(types::Event::ReasoningSummaryTextDelta {
                    delta: text.clone(),
                    item_id: reasoning.id.clone(),
                    output_index,
                    summary_index: summary_index as u64,
                });
            }

            events.push(types::Event::OutputItemDone {
                item: types::OutputItem::Reasoning(reasoning),
                output_index,
            });
            events
        }
        types::OutputItem::FunctionCall(function_call) => {
            // Mirror the streaming event sequence (added -> args delta -> done)
            // so `map_event` produces the same start/args/flush events it does
            // for a live stream.
            let arguments = function_call.arguments.clone();
            let item_id = function_call.id.clone().unwrap_or_default();
            vec![
                types::Event::OutputItemAdded {
                    item: types::OutputItem::FunctionCall(function_call.clone()),
                    output_index,
                },
                types::Event::FunctionCallArgumentsDelta {
                    delta: arguments,
                    item_id,
                    output_index,
                },
                types::Event::OutputItemDone {
                    item: types::OutputItem::FunctionCall(function_call),
                    output_index,
                },
            ]
        }
        types::OutputItem::FileSearch(_)
        | types::OutputItem::WebSearchResults(_)
        | types::OutputItem::ComputerToolCall(_) => vec![],
    }
}

fn map_non_streaming_finish_reason(
    status: types::ResponseStatus,
    incomplete_reason: Option<String>,
) -> Result<Event> {
    match status {
        types::ResponseStatus::Completed => Ok(Event::Finished(FinishReason::Completed)),
        types::ResponseStatus::Incomplete => {
            let reason = incomplete_reason.unwrap_or_else(|| "incomplete".to_owned());
            if reason == "max_output_tokens" || reason == "max_tokens" {
                return Ok(Event::Finished(FinishReason::MaxTokens));
            }

            Ok(Event::Finished(FinishReason::Other(reason.into())))
        }
        types::ResponseStatus::Failed => Err(Error::InvalidResponse(
            "OpenAI non-streaming response failed without a structured API error.".to_owned(),
        )),
        types::ResponseStatus::InProgress => Err(Error::InvalidResponse(
            "OpenAI non-streaming response did not complete.".to_owned(),
        )),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelListResponse {
    #[serde(rename = "object")]
    _object: String,
    pub data: Vec<ModelResponse>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelResponse {
    pub id: String,
    #[serde(rename = "object")]
    _object: String,
    #[serde(rename = "created", with = "chrono::serde::ts_seconds")]
    _created: chrono::DateTime<chrono::Utc>,
    #[serde(rename = "owned_by")]
    _owned_by: String,
}

#[cfg(test)]
impl Openai {
    /// Build the OpenAI wire request for `query` and serialize it to JSON
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
        let (request, ..) = create_request(model, query)?;
        Ok(serde_json::to_value(request)?)
    }
}

/// Create a request for the given model and query details.
///
/// Returns `(request, is_structured, reasoning_enabled)`.
#[expect(clippy::too_many_lines)]
fn create_request(model: &ModelDetails, query: ChatQuery) -> Result<(Request, bool, bool)> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
    } = query;

    let config = thread.events.config()?;
    let cache_policy = config.assistant.request.cache;
    let parameters = config.assistant.model.parameters;

    // Stable cache identity for this conversation. On load the stream's
    // creation timestamp is derived from the conversation ID, so every
    // request in a conversation produces the same key. A fork is a new
    // conversation with its own timestamp: its first request misses the
    // parent's warm cache and starts a cache lineage of its own.
    let conversation_created_at = thread.events.created_at;

    // Parse verbosity from the catch-all parameters map.
    let verbosity = parameters
        .other
        .get("verbosity")
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "low" => Some(types::TextVerbosity::Low),
            "medium" => Some(types::TextVerbosity::Medium),
            "high" => Some(types::TextVerbosity::High),
            _ => {
                warn!(verbosity = s, "Unknown verbosity value, ignoring.");
                None
            }
        });

    // Parse the OpenAI-specific reasoning execution mode from the catch-all
    // parameters map. `pro` performs more model work before returning a
    // single final answer, at the cost of latency and token usage.
    let reasoning_mode = parameters
        .other
        .get("reasoning_mode")
        .and_then(|v| v.as_str())
        .and_then(|s| parse_reasoning_mode(s, model));

    // Build the text config from structured output schema and/or verbosity.
    // Transform the schema for OpenAI's strict structured output mode.
    let text = match thread.events.schema() {
        Some(schema) => Some(types::TextConfig {
            format: types::TextFormat::JsonSchema {
                schema: {
                    let mut v = Value::Object(schema);
                    ensure_strict_schema(&mut v);
                    v
                },
                description: "Structured output".to_owned(),
                name: "structured_output".to_owned(),
                strict: Some(true),
            },
            verbosity,
        }),
        None => verbosity.map(|v| types::TextConfig {
            format: types::TextFormat::default(),
            verbosity: Some(v),
        }),
    };

    let is_structured = text.is_some();

    // Models with unknown reasoning support (absent from the catalog, e.g.
    // released after this binary was built) are assumed to support reasoning
    // request configuration. This lets `reasoning = "off"` send `effort: none`
    // rather than silently accepting the model's reasoning-on default.
    // Conversation replay is independent of this flag: namespaced OpenAI item
    // metadata determines whether a stored event has a native representation.
    let supports_reasoning = model
        .reasoning
        .is_none_or(|v| !matches!(v, ReasoningDetails::Unsupported));
    let mut reasoning = match model.custom_reasoning_config(parameters.reasoning) {
        Some(r) => Some(convert_reasoning(r, model)),
        // Explicitly disable reasoning for models that support it when the
        // user has turned it off. Sending `null` lets the model use its
        // default (which may include reasoning).
        //
        // For leveled models, use their lowest supported effort. Budgetted
        // models fall back to `minimal`, which every cataloged OpenAI
        // reasoning model accepts. Unknown models are newer than this binary,
        // and every OpenAI flagship since GPT-5.1 accepts `none`, so honor
        // "off" literally rather than spending reasoning tokens at the
        // model's default effort.
        None if supports_reasoning => {
            let effort = match model.reasoning {
                None => ReasoningEffort::None,
                Some(r) => r.lowest_effort().unwrap_or(ReasoningEffort::Xlow),
            };
            Some(convert_reasoning(
                CustomReasoningConfig {
                    effort,
                    exclude: true,
                },
                model,
            ))
        }
        None => None,
    };
    let reasoning_enabled = model
        .custom_reasoning_config(parameters.reasoning)
        .is_some();

    if reasoning_enabled && let Some(r) = reasoning.as_mut() {
        r.mode = reasoning_mode;

        // JP replays the complete event history — including encrypted
        // reasoning items — on every request, so ask supporting models to
        // render reasoning from earlier turns into the next sample.
        if model.features.contains(&PERSISTED_REASONING) {
            r.context = Some(types::ReasoningContext::AllTurns);
        }
    }

    let cache_enabled = !cache_policy.is_off();
    let explicit_cache = cache_enabled && model.features.contains(&EXPLICIT_PROMPT_CACHING);

    let parts = thread.into_parts();

    let mut messages = vec![];
    messages.push(to_system_messages(parts.system_parts, explicit_cache).0);

    // All attachments go in a user message before conversation events.
    let mut attachment_items = vec![];

    // Text attachments as XML to preserve source metadata.
    if let Some(xml) = text_attachments_to_xml(&parts.attachments)? {
        attachment_items.push(types::ContentItem::Text {
            text: xml,
            prompt_cache_breakpoint: None,
        });
    }

    // Binary attachments, each preceded by a label.
    for attachment in &parts.attachments {
        if let AttachmentContent::Binary { data, media_type } = &attachment.content {
            let b64 = base64::engine::general_purpose::STANDARD.encode(data);

            attachment_items.push(types::ContentItem::Text {
                text: format!("[Attached file: {}]", attachment.source),
                prompt_cache_breakpoint: None,
            });

            if media_type.starts_with("image/") {
                attachment_items.push(types::ContentItem::Image {
                    detail: types::ImageDetail::Auto,
                    file_id: None,
                    image_url: Some(format!("data:{media_type};base64,{b64}")),
                    prompt_cache_breakpoint: None,
                });
            } else if media_type == "application/pdf" {
                attachment_items.push(types::ContentItem::File {
                    file_data: Some(format!("data:{media_type};base64,{b64}")),
                    file_id: None,
                    filename: Some(attachment.source.clone()),
                    prompt_cache_breakpoint: None,
                });
            } else {
                warn!(
                    source = %attachment.source,
                    media_type,
                    "Unsupported binary attachment media type for OpenAI, skipping."
                );
            }
        }
    }

    if !attachment_items.is_empty() {
        // Extend the cached stable prefix (system prompt + attachments) up to
        // the last attachment block.
        if explicit_cache && let Some(item) = attachment_items.last_mut() {
            set_cache_breakpoint(item);
        }

        messages.push(types::InputListItem::Message(types::InputMessage {
            role: types::Role::User,
            content: types::ContentInput::List(attachment_items),
            phase: None,
        }));
    }

    // GPT-5 family models reject temperature/top_p when reasoning is active
    // (any effort other than `none`). Strip them and warn if configured.
    let strip_temp = model.features.contains(&TEMP_REQUIRES_NO_REASONING)
        && reasoning
            .as_ref()
            .is_some_and(|r| !matches!(r.effort, Some(types::ReasoningEffort::None)));
    let temperature = if strip_temp {
        if parameters.temperature.is_some() {
            warn!(
                model = %model.id,
                "temperature is not supported when reasoning is active; ignoring"
            );
        }
        None
    } else {
        parameters.temperature
    };
    let top_p = if strip_temp {
        if parameters.top_p.is_some() {
            warn!(
                model = %model.id,
                "top_p is not supported when reasoning is active; ignoring"
            );
        }
        None
    } else {
        parameters.top_p
    };

    messages.extend(convert_events(parts.events));
    let request = Request {
        model: types::Model::Other(model.id.name.to_string()),
        input: types::Input::List(messages),
        include: reasoning_enabled.then_some(vec![Include::ReasoningEncryptedContent]),
        store: Some(false),
        tool_choice: Some(convert_tool_choice(tool_choice)),
        tools: Some(convert_tools(tools)),
        temperature,
        reasoning,
        max_output_tokens: parameters.max_tokens.map(Into::into),
        truncation: Some(types::Truncation::Auto),
        top_p,
        text,
        // OpenAI routes requests by prompt prefix; a stable per-conversation
        // key improves cache-hit rates, and GPT-5.6+ models require it for
        // reliable cache matching.
        prompt_cache_key: cache_enabled.then(|| {
            format!(
                "jp:conversation:{}",
                conversation_created_at.timestamp_micros()
            )
        }),
        // Explicit mode with no marked breakpoints disables cache reads and
        // writes; the only way to opt out of caching on models that bill
        // cache writes. Models without the feature flag cache automatically
        // and at no extra cost, so there is nothing to disable for them.
        prompt_cache_options: (cache_policy.is_off()
            && model.features.contains(&EXPLICIT_PROMPT_CACHING))
        .then_some(types::PromptCacheOptions {
            mode: Some(types::PromptCacheMode::Explicit),
            ttl: None,
        }),
        ..Default::default()
    };

    debug!("Sending request to OpenAI.");
    trace!(
        request = %trace_to_tmpfile("jp-openai-request", &request),
        "Request payload."
    );

    Ok((request, is_structured, reasoning_enabled))
}

#[expect(clippy::too_many_lines)]
fn map_model(model: ModelResponse) -> Result<ModelDetails> {
    let details = match model.id.as_str() {
        "gpt-5.6" | "gpt-5.6-sol" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.6 Sol".to_owned()),
            context_window: Some(1_050_000),
            max_output_tokens: Some(128_000),
            // Reasoning.effort supports: none, low, medium, high, xhigh, max.
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, true,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2026, 2, 16).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![
                TEMP_REQUIRES_NO_REASONING,
                REASONING_PRO_MODE,
                PERSISTED_REASONING,
                EXPLICIT_PROMPT_CACHING,
            ],
        },
        "gpt-5.6-terra" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.6 Terra".to_owned()),
            context_window: Some(1_050_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, true,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2026, 2, 16).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![
                TEMP_REQUIRES_NO_REASONING,
                REASONING_PRO_MODE,
                PERSISTED_REASONING,
                EXPLICIT_PROMPT_CACHING,
            ],
        },
        "gpt-5.6-luna" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.6 Luna".to_owned()),
            context_window: Some(1_050_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, true,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2026, 2, 16).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![
                TEMP_REQUIRES_NO_REASONING,
                REASONING_PRO_MODE,
                PERSISTED_REASONING,
                EXPLICIT_PROMPT_CACHING,
            ],
        },
        "gpt-5.5" | "gpt-5.5-2026-04-23" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.5".to_owned()),
            context_window: Some(1_050_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 12, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.5-pro" | "gpt-5.5-pro-2026-04-23" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.5 pro".to_owned()),
            context_window: Some(1_050_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                false, false, false, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 12, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING, STREAMING_UNSUPPORTED],
        },
        "gpt-5.4" | "gpt-5.4-2026-03-05" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.4".to_owned()),
            context_window: Some(1_050_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.4-pro" | "gpt-5.4-pro-2026-03-05" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.4 pro".to_owned()),
            context_window: Some(1_050_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                false, false, false, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.4-mini" | "gpt-5.4-mini-2026-03-17" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.4 mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.4-nano" | "gpt-5.4-nano-2026-03-17" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.4 nano".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.3-codex" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.3 Codex".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                false, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.3-chat-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.3 Chat".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::leveled(
                false, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 8, 10).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.2-codex" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.2 Codex".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            // Reasoning.effort supports: low, medium, high, xhigh (no none)
            reasoning: Some(ReasoningDetails::leveled(
                false, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.2-pro" | "gpt-5.2-pro-2025-12-11" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.2 pro".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                false, false, false, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.2" | "gpt-5.2-2025-12-11" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.2".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            // Reasoning.effort supports: none (default), low, medium, high, xhigh
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.2-chat-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.2 Chat".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 8, 31).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 8, 10).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.1-codex-max" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1-Codex-Max".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.1-codex" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1 Codex".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.1-codex-mini" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1 Codex mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.4-mini",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.1" | "gpt-5.1-2025-11-13" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            // Reasoning.effort supports: none (default), low, medium, high
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, false, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5.1-chat-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1 Chat".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::leveled(
                true, false, true, true, true, false, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5-codex" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5-Codex".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5" | "gpt-5-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            // Reasoning.effort supports: minimal, low, medium, high
            reasoning: Some(ReasoningDetails::leveled(
                false, true, true, true, true, false, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 12, 11).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5-pro" | "gpt-5-pro-2025-10-06" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 pro".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(
                false, false, false, false, true, false, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5-pro",
                Some(NaiveDate::from_ymd_opt(2026, 12, 11).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5-chat-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 Chat".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::leveled(
                false, true, true, true, true, false, false,
            )),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 9, 30).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5-mini" | "gpt-5-mini-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 5, 31).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.4-mini",
                Some(NaiveDate::from_ymd_opt(2026, 12, 11).unwrap()),
            )),
            structured_output: None,
            features: vec![TEMP_REQUIRES_NO_REASONING],
        },
        "gpt-5-nano" | "gpt-5-nano-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 nano".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 5, 31).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.4-nano",
                Some(NaiveDate::from_ymd_opt(2026, 12, 11).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "o4-mini" | "o4-mini-2025-04-16" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o4-mini".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.4-mini",
                Some(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "o3-mini" | "o3-mini-2025-01-31" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-mini".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "o3" | "o3-2025-04-16" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 12, 11).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "o3-pro" | "o3-pro-2025-06-10" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-pro".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5-pro",
                Some(NaiveDate::from_ymd_opt(2026, 12, 11).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "o1" | "o1-2024-12-17" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                Some(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "o1-pro" | "o1-pro-2025-03-19" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1-pro".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5-pro",
                Some(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "gpt-4.1" | "gpt-4.1-2025-04-14" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4.1".to_owned()),
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![],
        },
        "gpt-4o" | "gpt-4o-2024-08-06" | "gpt-4o-2024-11-20" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4o".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            // Deprecated without an announced retirement date; only the
            // 2024-05-13 snapshot has a scheduled shutdown (2026-10-23).
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5",
                None,
            )),
            structured_output: None,
            features: vec![],
        },
        "gpt-4.1-nano" | "gpt-4.1-nano-2025-04-14" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4.1 nano".to_owned()),
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.4-nano",
                Some(NaiveDate::from_ymd_opt(2026, 10, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "gpt-4o-mini" | "gpt-4o-mini-2024-07-18" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4o mini".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![],
        },
        "gpt-4.1-mini" | "gpt-4.1-mini-2025-04-14" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4.1 mini".to_owned()),
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![],
        },
        "gpt-oss-120b" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("gpt-oss-120b".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(131_072),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![],
        },
        "gpt-oss-20b" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("gpt-oss-20b".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(131_072),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            structured_output: None,
            features: vec![],
        },
        "o3-deep-research" | "o3-deep-research-2025-06-26" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-deep-research".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5-pro",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        "o4-mini-deep-research" | "o4-mini-deep-research-2025-06-26" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o4-mini-deep-research".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.5-pro",
                Some(NaiveDate::from_ymd_opt(2026, 7, 23).unwrap()),
            )),
            structured_output: None,
            features: vec![],
        },
        id => {
            warn!(model = id, ?model, "Missing model details.");
            ModelDetails::empty((PROVIDER, id).try_into()?)
        }
    };

    Ok(details)
}

/// Filter out unknown event types from the OpenAI SSE stream.
///
/// OpenAI may introduce new streaming event types (e.g., `keepalive`) that the
/// `openai_responses` crate doesn't know about yet.
/// These cause deserialization failures.
/// Rather than killing the stream, we silently skip them.
async fn skip_unknown_events(
    result: std::result::Result<types::Event, OpenaiStreamError>,
) -> Option<std::result::Result<types::Event, OpenaiStreamError>> {
    if let Err(OpenaiStreamError::Parsing(e)) = &result
        && e.to_string().starts_with("unknown variant")
    {
        trace!("Skipping unknown OpenAI streaming event: {e}");
        return None;
    }
    Some(result)
}

/// Convert an OpenAI [`OpenaiStreamError`] into a [`StreamError`].
async fn map_error(error: OpenaiStreamError) -> std::result::Result<types::Event, StreamError> {
    Err(match error {
        OpenaiStreamError::Stream(error) => StreamError::from_eventsource(error).await,
        OpenaiStreamError::Parsing(error) => {
            StreamError::other(error.to_string()).with_source(error)
        }
    })
}

/// Map an Openai [`types::Event`] into one or more [`Event`]s.
#[expect(clippy::too_many_lines)]
fn map_event(
    event: types::Event,
    is_structured: bool,
    reasoning_enabled: bool,
) -> Vec<std::result::Result<Event, StreamError>> {
    use types::Event::*;

    trace!(
        event = serde_json::to_string(&event).unwrap_or_default(),
        "Received event from OpenAI API."
    );

    #[expect(clippy::cast_possible_truncation)]
    match event {
        // We emit an empty message first, because sometimes the API returns
        // empty messages which produce no `OutputTextDelta` events. In such a
        // case, we would emit NO `Event::Part` events, but WOULD emit a `flush`
        // event, which is not what we want. To avoid this, we *ALWAYS* emit a
        // `Event::Part` event, even if the message is empty.
        OutputItemAdded {
            output_index,
            item: types::OutputItem::Message(_),
        } => vec![Ok(if is_structured {
            Event::structured(output_index as usize, String::new())
        } else {
            Event::message(output_index as usize, String::new())
        })],

        // Skip all reasoning events when reasoning is disabled. The model
        // may still return minimal reasoning output at `effort: "minimal"`.
        OutputItemAdded {
            item: types::OutputItem::Reasoning(_),
            ..
        }
        | ReasoningSummaryTextDelta { .. }
        | OutputItemDone {
            item: types::OutputItem::Reasoning(_),
            ..
        } if !reasoning_enabled => vec![],

        OutputItemAdded {
            output_index,
            item: types::OutputItem::Reasoning(_),
        } => vec![Ok(Event::reasoning(output_index as usize, String::new()))],

        // Emit the tool-call start as soon as the item is announced, before any
        // arguments stream in. This renders the call header early and opens the
        // tool-call window for the keep-alive layer to fill the silent gap
        // while the model generates the arguments.
        OutputItemAdded {
            output_index,
            item: types::OutputItem::FunctionCall(types::FunctionCall { call_id, name, .. }),
        } => vec![Ok(Event::tool_call_start(
            output_index as usize,
            &call_id,
            &name,
        ))],

        OutputTextDelta {
            delta,
            output_index,
            ..
        }
        | RefusalDelta {
            delta,
            output_index,
            ..
        } => {
            let index = output_index as usize;
            vec![Ok(if is_structured {
                Event::structured(index, delta)
            } else {
                Event::message(index, delta)
            })]
        }

        ReasoningSummaryTextDelta {
            delta,
            output_index,
            ..
        } => vec![Ok(Event::reasoning(output_index as usize, delta))],

        FunctionCallArgumentsDelta {
            delta,
            output_index,
            ..
        } => vec![Ok(Event::tool_call_args(output_index as usize, delta))],

        OutputItemDone { item, output_index } => {
            let index = output_index as usize;
            let mut events = vec![];
            let metadata = match &item {
                types::OutputItem::FunctionCall(_) => Map::new(),
                types::OutputItem::Message(v) => {
                    let mut map = Map::new();
                    map.insert(ITEM_ID_KEY.to_owned(), v.id.clone().into());
                    if let Some(phase) = &v.phase {
                        let phase_str = match phase {
                            types::Phase::Commentary => "commentary",
                            types::Phase::FinalAnswer => "final_answer",
                        };
                        map.insert(PHASE_KEY.to_owned(), phase_str.into());
                    }
                    map
                }
                types::OutputItem::Reasoning(v) => {
                    let mut map = Map::new();
                    map.insert(ITEM_ID_KEY.into(), v.id.clone().into());
                    map.insert(
                        ENCRYPTED_CONTENT_KEY.into(),
                        v.encrypted_content.clone().into(),
                    );
                    map
                }

                // We don't handle these output items for now.
                types::OutputItem::FileSearch(_)
                | types::OutputItem::WebSearchResults(_)
                | types::OutputItem::ComputerToolCall(_) => return vec![],
            };

            // A function call's start and argument chunks are emitted from the
            // `OutputItemAdded` and `FunctionCallArgumentsDelta` events; only
            // the flush remains to be emitted here.
            events.push(Ok(Event::flush_with_metadata(index, metadata)));
            events
        }
        // Terminal lifecycle events. Emit `Finished` from the real protocol
        // signal so the stream carries its own completion: a stream that ends
        // without one (a dropped connection) is treated as incomplete by the
        // consumer and retried, rather than being mistaken for a clean finish.
        ResponseCompleted { response }
        | ResponseIncomplete { response }
        | ResponseFailed { response } => {
            let incomplete_reason = response.incomplete_details.map(|d| d.reason);
            match map_non_streaming_finish_reason(response.status, incomplete_reason) {
                Ok(event) => vec![Ok(event)],
                Err(error) => vec![Err(StreamError::other(error.to_string()))],
            }
        }
        Error { error } => vec![Err(classify_stream_error(error))],
        _ => vec![],
    }
}

/// Classify an OpenAI streaming error event into a [`StreamError`].
///
/// Maps well-known error types (quota, rate-limit, auth, server errors) to the
/// appropriate [`StreamErrorKind`] so the retry and display layers can handle
/// them correctly.
///
/// In-stream errors carry no HTTP headers, so `retry_after` timing comes solely
/// from message-body parsing (e.g. `"Please try again in 2.398s."`).
///
/// Classification checks both `type` and `code` because OpenAI's per-minute
/// token/request rate-limits arrive as `type=tokens|requests` with
/// `code=rate_limit_exceeded` — the real signal is in `code`.
fn classify_stream_error(error: types::response::Error) -> StreamError {
    let retry_after = extract_retry_from_text(&error.message);
    let code = error.code.as_deref();
    let type_ = error.r#type.as_str();

    // Quota exhaustion is a hard-stop; check this before rate-limit because
    // both the type and code can overlap with retryable categories.
    if code == Some("insufficient_quota")
        || type_ == "insufficient_quota"
        || looks_like_quota_error(&error.message)
    {
        return StreamError::new(
            StreamErrorKind::InsufficientQuota,
            format!(
                "Insufficient API quota. Check your plan and billing details \
                 at https://platform.openai.com/settings/organization/billing. \
                 ({})",
                error.message
            ),
        );
    }

    // Rate-limits may signal via either `type` or `code`. OpenAI's TPM/RPM
    // in-stream limits use `type=tokens|requests` with `code=rate_limit_exceeded`.
    if code == Some("rate_limit_exceeded") || type_ == "rate_limit_exceeded" {
        return StreamError::rate_limit(retry_after);
    }

    // Server-side transient errors. Match on either type or code: OpenAI emits
    // generic types (`server_error`, `api_error`) but also more specific overload
    // signals where the discriminator lives in `code` (e.g.
    // `service_unavailable_error` / `server_is_overloaded`, the wire form of
    // the documented 503 "engine is currently overloaded" condition). Some
    // OpenAI-compatible providers also reuse Anthropic's `overloaded_error`.
    let transient_type = matches!(
        type_,
        "server_error" | "api_error" | "service_unavailable_error" | "overloaded_error"
    );
    let transient_code = matches!(code, Some("server_is_overloaded"));
    if transient_type || transient_code {
        let err = StreamError::transient(error.message);
        return match retry_after {
            Some(d) => err.with_retry_after(d),
            None => err,
        };
    }

    // Catch-all. If the message carries a retry hint we can honor, treat it
    // as transient so the retry layer still engages; otherwise surface as a
    // generic error.
    let display = format!(
        "OpenAI error: type={}, code={:?}, message={}, param={:?}",
        error.r#type, error.code, error.message, error.param
    );

    match retry_after {
        Some(d) => StreamError::transient(display).with_retry_after(d),
        None => StreamError::other(display),
    }
}

impl TryFrom<&OpenaiConfig> for Openai {
    type Error = Error;

    fn try_from(config: &OpenaiConfig) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        let reqwest_client = reqwest::Client::builder()
            .default_headers(HeaderMap::from_iter([(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}"))
                    .map_err(|_| CreateError::InvalidApiKey)?,
            )]))
            .build()?;

        let base_url =
            std::env::var(&config.base_url_env).unwrap_or_else(|_| config.base_url.clone());

        let client = Client::new(&api_key)?.with_base_url(base_url);

        Ok(Openai {
            reqwest_client,
            client,
            base_url: config.base_url.clone(),
        })
    }
}

/// Transforms a JSON schema in place into OpenAI's strict-mode shape.
///
/// At each node of the schema tree:
///
/// - Objects get `additionalProperties: false` and every property listed in
///   `required`.
/// - Properties that were not originally in `required` are made nullable (via
///   [`make_schema_nullable`]) so the model can emit `null` to omit them —
///   preserving the schema author's intent that those fields are optional (per
///   OpenAI's docs: "it is possible to emulate an optional parameter by using a
///   union type with null").
/// - Recursion descends into every property value, into `items`, into each
///   `anyOf` variant, into `$defs`/`definitions`, and into the entries of an
///   `allOf`.
/// - `allOf` is flattened into the parent schema (OpenAI's strict mode doesn't
///   accept composition keywords).
/// - `null` defaults are stripped (no meaningful distinction in strict mode).
/// - A `$ref` with sibling properties is unravelled by inlining the resolved
///   definition and re-running on the merged result (OpenAI supports standalone
///   `$ref` but not alongside other keys).
///
/// Mirrors the recursion pattern of OpenAI's own SDK helper
/// (`_ensure_strict_json_schema` in `openai-python`), with two intentional
/// differences:
///
/// 1. The Python SDK only sets `additionalProperties: false` if it's missing;
///    we overwrite even if it's `true`.
///    Forgiving rather than rejecting an upstream mistake that OpenAI would
///    otherwise refuse.
/// 2. The Python SDK doesn't inject nullability — Pydantic emits the `anyOf:
///    [..., null]` form upstream.
///    Our function-calling pipeline goes through `ToolParameterConfig` which
///    encodes optionality as `required: bool`, so we have to bridge that here.
///
/// See: <https://platform.openai.com/docs/guides/structured-outputs>
fn ensure_strict_schema(schema: &mut Value) {
    let root = schema.clone();
    process_strict(schema, &root);
}

fn process_strict(schema: &mut Value, root: &Value) {
    let Value::Object(map) = schema else {
        return;
    };

    // 1. Recurse into $defs / definitions.
    for key in ["$defs", "definitions"] {
        if let Some(Value::Object(defs)) = map.get_mut(key) {
            for def_schema in defs.values_mut() {
                process_strict(def_schema, root);
            }
        }
    }

    // 2. Strict object treatment + nullability injection for any
    //    previously-optional properties.
    if is_object_type(map.get("type")) {
        map.insert("additionalProperties".to_owned(), false.into());

        if let Some(Value::Object(props)) = map.get("properties") {
            let prev_required: Vec<String> = map
                .get("required")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();

            let newly_required: Vec<String> = props
                .keys()
                .filter(|k| !prev_required.iter().any(|r| r == *k))
                .cloned()
                .collect();

            let all_keys: Vec<Value> = props.keys().map(|k| Value::String(k.clone())).collect();
            map.insert("required".to_owned(), Value::Array(all_keys));

            // Make previously-optional properties nullable *before*
            // recursing into them, so nullability lands on the typed
            // variant inside any resulting `anyOf` wrapper and the
            // recursion then descends into that variant.
            if let Some(Value::Object(props)) = map.get_mut("properties") {
                for key in &newly_required {
                    if let Some(prop_schema) = props.get_mut(key) {
                        make_schema_nullable(prop_schema);
                    }
                }
            }
        }
    }

    // 3. Recurse into property values, items, and anyOf variants.
    if let Some(Value::Object(props)) = map.get_mut("properties") {
        for prop_schema in props.values_mut() {
            process_strict(prop_schema, root);
        }
    }
    if let Some(items) = map.get_mut("items") {
        process_strict(items, root);
    }
    if let Some(Value::Array(variants)) = map.get_mut("anyOf") {
        for variant in variants.iter_mut() {
            process_strict(variant, root);
        }
    }

    // 4. Flatten `allOf` into the parent. Earlier entries (and keys
    //    already on the parent) take precedence. OpenAI's strict mode
    //    rejects composition keywords, so even multi-entry `allOf`
    //    must collapse.
    if let Some(Value::Array(entries)) = map.remove("allOf") {
        for mut entry in entries {
            process_strict(&mut entry, root);
            if let Value::Object(entry_map) = entry {
                for (k, v) in entry_map {
                    map.entry(k).or_insert(v);
                }
            }
        }
    }

    // 5. Strip `null` defaults.
    if map.get("default") == Some(&Value::Null) {
        map.remove("default");
    }

    // 6. Unravel `$ref` with siblings. OpenAI supports standalone
    //    `$ref` but not alongside other keys.
    if map.contains_key("$ref")
        && map.len() > 1
        && let Some(Value::String(ref_path)) = map.remove("$ref")
    {
        if let Some(resolved) = resolve_ref(&ref_path, root) {
            // Current schema's keys take priority over the
            // resolved definition's.
            let mut merged = resolved;
            for (k, v) in std::mem::take(map) {
                merged.insert(k, v);
            }
            *map = merged;
            // Re-run on the inlined result.
            process_strict(schema, root);
            return;
        }
        // Failed to resolve — put it back.
        map.insert("$ref".to_owned(), Value::String(ref_path));
    }
}

/// Resolve a JSON pointer against the root schema.
///
/// Handles paths like `#/$defs/MyType` and `#` (root self-reference).
fn resolve_ref(ref_path: &str, root: &Value) -> Option<Map<String, Value>> {
    if ref_path == "#" {
        return root.as_object().cloned();
    }

    let path = ref_path.strip_prefix("#/")?;
    let mut current = root;
    for segment in path.split('/') {
        current = current.get(segment)?;
    }
    current.as_object().cloned()
}

fn convert_tool_choice(choice: ToolChoice) -> types::ToolChoice {
    match choice {
        ToolChoice::Auto => types::ToolChoice::Auto,
        ToolChoice::None => types::ToolChoice::None,
        ToolChoice::Required => types::ToolChoice::Required,
        ToolChoice::Function(name) => types::ToolChoice::Function(name),
    }
}

pub(crate) fn parameters_with_strict_mode(
    parameters: IndexMap<String, ToolParameterConfig>,
    strict: bool,
) -> Map<String, Value> {
    let required = parameters
        .iter()
        .filter(|(_, cfg)| strict || cfg.required)
        .map(|(k, _)| k.clone())
        .collect::<Vec<_>>();

    let properties = parameters
        .into_iter()
        .map(|(k, mut cfg)| {
            sanitize_parameter(&mut cfg);
            let needs_null = strict && !cfg.required;

            let mut schema = cfg.to_json_schema();

            if needs_null {
                make_schema_nullable(&mut schema);
            }

            // If `strict` mode is enabled, the schema for each parameter
            // must satisfy: `additionalProperties: false` on every
            // object, all fields in `required`, and optional fields
            // emulated via nullability.
            //
            // See: <https://platform.openai.com/docs/guides/function-calling#strict-mode>
            if strict {
                ensure_strict_schema(&mut schema);
            }

            (k, schema)
        })
        .collect::<Map<_, _>>();

    Map::from_iter([
        ("type".to_owned(), "object".into()),
        ("properties".to_owned(), properties.into()),
        ("additionalProperties".to_owned(), (!strict).into()),
        ("required".to_owned(), required.into()),
    ])
}

/// Check whether a JSON schema `type` value includes `"object"`.
///
/// Handles both `"object"` (string) and `["object", "null"]` (array) forms that
/// arise after nullable injection.
fn is_object_type(type_value: Option<&Value>) -> bool {
    match type_value {
        Some(Value::String(s)) => s == "object",
        Some(Value::Array(arr)) => arr.iter().any(|v| v.as_str() == Some("object")),
        _ => false,
    }
}

/// Keys that stay at the outer level when wrapping a structured schema in
/// `anyOf` for nullability.
/// Everything else moves into the typed variant alongside
/// `type`/`items`/`properties`.
const NULLABLE_OUTER_KEYS: &[&str] = &["description", "title", "default", "examples"];

/// Injects nullability into a raw JSON schema value.
///
/// The encoding depends on the underlying type:
///
/// - **Primitives** (`string`, `integer`, `number`, `boolean`) extend their
///   `type` field to include `"null"` — `{"type": "string"}` becomes `{"type":
///   ["string", "null"]}`.
///   This matches OpenAI's documented optional-field example for strict mode.
/// - **Structured types** (`array`, `object`) wrap the typed schema in `anyOf`,
///   lifting descriptive metadata out to the outer level — `{"type": "array",
///   "items": ..., "description": "..."}` becomes `{"anyOf": [{"type": "array",
///   "items": ...}, {"type": "null"}], "description": "..."}`.
///   OpenAI's strict validator rejects the `type` array form for structured
///   types because it treats the sibling constraints (`items`, `properties`) as
///   orphaned from the typed variant; Pydantic (OpenAI's own SDK) emits the
///   same `anyOf` shape for `Optional[List[T]]` and `Optional[BaseModel]`.
///
/// Idempotent: a schema that's already nullable (via either encoding) is
/// returned unchanged.
fn make_schema_nullable(schema: &mut Value) {
    let Value::Object(map) = schema else {
        return;
    };

    let Some(type_val) = map.get("type").cloned() else {
        // No `type` key — could be `anyOf` (already nullable) or `$ref`.
        // Nothing for us to inject here.
        return;
    };

    let already_nullable = match &type_val {
        Value::String(t) => t == "null",
        Value::Array(arr) => arr.iter().any(|v| v.as_str() == Some("null")),
        _ => false,
    };
    if already_nullable {
        return;
    }

    let is_structured = |t: &str| matches!(t, "array" | "object");
    let needs_anyof = match &type_val {
        Value::String(t) => is_structured(t),
        Value::Array(arr) => arr.iter().any(|v| v.as_str().is_some_and(is_structured)),
        _ => false,
    };

    if !needs_anyof {
        match type_val {
            Value::String(t) => {
                map.insert(
                    "type".to_owned(),
                    Value::Array(vec![Value::String(t), "null".into()]),
                );
            }
            Value::Array(mut arr) => {
                arr.push("null".into());
                map.insert("type".to_owned(), Value::Array(arr));
            }
            _ => {}
        }
        return;
    }

    // Structured: split the schema into a typed variant (everything
    // that's a constraint on the value) and an outer wrapper carrying
    // descriptive metadata.
    let owned = std::mem::take(map);
    let (outer, inner): (Map<String, Value>, Map<String, Value>) = owned
        .into_iter()
        .partition(|(k, _)| NULLABLE_OUTER_KEYS.contains(&k.as_str()));

    map.extend(outer);
    map.insert(
        "anyOf".to_owned(),
        Value::Array(vec![
            Value::Object(inner),
            Value::Object(Map::from_iter([("type".to_owned(), "null".into())])),
        ]),
    );
}

/// Sanitizes the parameter shape to fit Openai's limitations. specifically
/// moving array-based enums into the 'items' configuration.
fn sanitize_parameter(config: &mut ToolParameterConfig) {
    if let Some(items) = &mut config.items {
        sanitize_parameter(items);
    }

    let allows_array = match &config.kind {
        OneOrManyTypes::One(t) => t == "array",
        OneOrManyTypes::Many(types) => types.iter().any(|t| t == "array"),
    };

    if !allows_array || !config.enumeration.iter().any(Value::is_array) {
        return;
    }

    let (arrays, other): (Vec<Value>, Vec<Value>) =
        config.enumeration.drain(..).partition(Value::is_array);

    config.enumeration = other;

    // Flatten [["foo", "bar"], ["baz"]] -> ["foo", "bar", "baz"]
    let items: Vec<Value> = arrays
        .into_iter()
        .flat_map(|v| match v {
            Value::Array(v) => v,
            _ => vec![],
        })
        .collect();

    let items_config = config.items.get_or_insert_with(|| {
        let mut inferred_types: IndexSet<_> = items
            .iter()
            .map(|v| match v {
                Value::String(_) => "string",
                Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                Value::Null => "null",
                Value::Object(_) => "object",
                Value::Array(_) => "array",
            })
            .map(str::to_owned)
            .collect();

        // Construct the correct kind
        let kind = if inferred_types.len() == 1
            && let Some(first) = inferred_types.pop()
        {
            OneOrManyTypes::One(first)
        } else {
            OneOrManyTypes::Many(inferred_types.into_iter().collect())
        };

        Box::new(ToolParameterConfig {
            kind,
            default: None,
            required: false,
            summary: None,
            description: None,
            examples: None,
            enumeration: vec![],
            items: None,
            properties: IndexMap::default(),
        })
    });

    // Append the flattened values to the items enum
    items_config.enumeration.extend(items);
}

fn convert_tools(tools: Vec<ToolDefinition>) -> Vec<types::Tool> {
    tools
        .into_iter()
        .map(|tool| types::Tool::Function {
            name: tool.name,
            strict: true,
            description: tool.docs.schema_description().map(str::to_owned),
            parameters: parameters_with_strict_mode(tool.parameters, true).into(),
        })
        .collect()
}

/// Parse an OpenAI reasoning execution mode value (`standard` or `pro`).
///
/// `pro` is returned only when the model supports it; unsupported models fall
/// back to standard mode, with a warning.
/// `standard` returns `None`, since it is the API default.
fn parse_reasoning_mode(value: &str, model: &ModelDetails) -> Option<types::ReasoningMode> {
    match value {
        "standard" => None,
        "pro" if model.features.contains(&REASONING_PRO_MODE) => Some(types::ReasoningMode::Pro),
        "pro" => {
            warn!(
                model = %model.id,
                "Model does not support pro reasoning mode; using standard mode."
            );
            None
        }
        _ => {
            warn!(
                reasoning_mode = value,
                "Unknown reasoning_mode value, ignoring."
            );
            None
        }
    }
}

/// Convert the reasoning configuration to the OpenAI wire format.
///
/// The `max` effort is only sent to models that support it; others degrade to
/// `xhigh`.
fn convert_reasoning(
    reasoning: CustomReasoningConfig,
    model: &ModelDetails,
) -> types::ReasoningConfig {
    let supports_max = model.reasoning.is_some_and(|r| r.supports_max_effort());

    // Always request reasoning summaries so they're captured in the
    // conversation. The display layer handles visibility.
    types::ReasoningConfig {
        summary: Some(SummaryConfig::Auto),
        effort: match reasoning
            .effort
            .abs_to_rel(model.max_output_tokens)
            .unwrap_or(ReasoningEffort::Auto)
        {
            ReasoningEffort::None => Some(types::ReasoningEffort::None),
            ReasoningEffort::Max if supports_max => Some(types::ReasoningEffort::Max),
            ReasoningEffort::Max | ReasoningEffort::XHigh => Some(types::ReasoningEffort::XHigh),
            ReasoningEffort::High => Some(types::ReasoningEffort::High),
            ReasoningEffort::Auto | ReasoningEffort::Medium => Some(types::ReasoningEffort::Medium),
            ReasoningEffort::Low => Some(types::ReasoningEffort::Low),
            ReasoningEffort::Xlow => Some(types::ReasoningEffort::Minimal),
            ReasoningEffort::Absolute(_) => {
                debug_assert!(false, "Reasoning effort must be relative.");
                None
            }
        },
        mode: None,
        context: None,
    }
}

struct ListItem(types::InputListItem);

impl IntoIterator for ListItem {
    type Item = types::InputListItem;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![self.0].into_iter()
    }
}

fn to_system_messages(parts: Vec<String>, cache_breakpoint: bool) -> ListItem {
    let mut items: Vec<_> = parts
        .into_iter()
        .map(|text| types::ContentItem::Text {
            text,
            prompt_cache_breakpoint: None,
        })
        .collect();

    // Cache the system-prompt prefix. The growing conversation tail is
    // covered by the implicit breakpoint on the latest message.
    if cache_breakpoint && let Some(item) = items.last_mut() {
        set_cache_breakpoint(item);
    }

    ListItem(types::InputListItem::Message(types::InputMessage {
        role: types::Role::System,
        content: types::ContentInput::List(items),
        phase: None,
    }))
}

/// Mark a content block as the end of a cacheable prompt prefix.
///
/// The breakpoint covers the block itself and all prompt content rendered
/// before it; content after it can change without invalidating the cached
/// prefix.
fn set_cache_breakpoint(item: &mut types::ContentItem) {
    let (types::ContentItem::Text {
        prompt_cache_breakpoint,
        ..
    }
    | types::ContentItem::Image {
        prompt_cache_breakpoint,
        ..
    }
    | types::ContentItem::File {
        prompt_cache_breakpoint,
        ..
    }) = item;

    *prompt_cache_breakpoint = Some(types::PromptCacheBreakpoint {
        mode: types::PromptCacheBreakpointMode::Explicit,
    });
}

/// Parse a phase string from metadata into the API type.
fn parse_phase(metadata: &mut Map<String, Value>) -> Option<types::Phase> {
    metadata
        .remove(PHASE_KEY)
        .and_then(|v| v.as_str().map(str::to_owned))
        .and_then(|s| match s.as_str() {
            "commentary" => Some(types::Phase::Commentary),
            "final_answer" => Some(types::Phase::FinalAnswer),
            _ => None,
        })
}

#[expect(clippy::too_many_lines)]
fn convert_events(events: ConversationStream) -> Vec<types::InputListItem> {
    events
        .into_iter()
        .flat_map(|event| {
            let ConversationEvent {
                kind, mut metadata, ..
            } = event.event;

            match kind {
                EventKind::ChatRequest(request) => {
                    vec![types::InputListItem::Message(types::InputMessage {
                        role: types::Role::User,
                        content: types::ContentInput::Text(request.content),
                        phase: None,
                    })]
                }
                EventKind::ChatResponse(response) => {
                    let id = metadata
                        .remove(ITEM_ID_KEY)
                        .and_then(|v| v.as_str().map(str::to_owned));

                    let encrypted_content = metadata
                        .remove(ENCRYPTED_CONTENT_KEY)
                        .and_then(|v| v.as_str().map(str::to_owned));

                    let phase = parse_phase(&mut metadata);

                    match response {
                        ChatResponse::Reasoning { reasoning } => {
                            if let Some(id) = id {
                                // The namespaced OpenAI item id is proof that
                                // this event originated as a native reasoning
                                // item. Preserve that representation and let
                                // the Responses API decide whether it is
                                // compatible with the target model.
                                vec![types::InputListItem::Item(types::InputItem::Reasoning(
                                    types::Reasoning {
                                        id,
                                        summary: vec![types::ReasoningSummary::Text {
                                            text: reasoning,
                                        }],
                                        encrypted_content,
                                        status: None,
                                    },
                                ))]
                            } else {
                                // Reasoning from another provider has no
                                // OpenAI-native representation.
                                vec![types::InputListItem::Message(types::InputMessage {
                                    role: types::Role::Assistant,
                                    content: types::ContentInput::Text(format!(
                                        "<think>\n{reasoning}\n</think>\n\n",
                                    )),
                                    phase,
                                })]
                            }
                        }
                        ChatResponse::Message { message } => {
                            if let Some(id) = id {
                                vec![types::InputListItem::Item(types::InputItem::OutputMessage(
                                    types::OutputMessage {
                                        id,
                                        role: types::Role::Assistant,
                                        content: vec![types::OutputContent::Text {
                                            text: message,
                                            annotations: vec![],
                                        }],
                                        status: types::MessageStatus::Completed,
                                        phase,
                                    },
                                ))]
                            } else {
                                vec![types::InputListItem::Message(types::InputMessage {
                                    role: types::Role::Assistant,
                                    content: types::ContentInput::Text(message),
                                    phase,
                                })]
                            }
                        }
                        ChatResponse::Structured { data } => {
                            vec![types::InputListItem::Message(types::InputMessage {
                                role: types::Role::Assistant,
                                content: types::ContentInput::Text(data.to_string()),
                                phase,
                            })]
                        }
                    }
                }
                EventKind::ToolCallRequest(request) => vec![types::InputListItem::Item(
                    types::InputItem::FunctionCall(types::FunctionCall {
                        call_id: request.id,
                        name: request.name,
                        arguments: Value::Object(request.arguments).to_string(),
                        status: None,
                        id: None,
                    }),
                )],
                EventKind::ToolCallResponse(ToolCallResponse { id, result }) => {
                    vec![types::InputListItem::Item(
                        types::InputItem::FunctionCallOutput(types::FunctionCallOutput {
                            call_id: id,
                            output: match result {
                                Ok(content) | Err(content) => content,
                            },
                            id: None,
                            status: None,
                        }),
                    )]
                }
                _ => vec![],
            }
        })
        .collect()
}

impl From<types::response::Error> for Error {
    fn from(error: types::response::Error) -> Self {
        Self::OpenaiResponse(error)
    }
}

#[cfg(test)]
#[path = "openai_tests.rs"]
mod tests;
