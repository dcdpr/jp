use std::env;

use async_trait::async_trait;
use chrono::NaiveDate;
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _, future, stream};
use indexmap::{IndexMap, IndexSet};
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
    event::{ChatResponse, ConversationEvent, EventKind, ToolCallRequest, ToolCallResponse},
};
use openai_responses::{
    Client, CreateError, StreamError as OpenaiStreamError,
    types::{self, Include, Request, SummaryConfig},
};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Map, Value};
use tracing::{trace, warn};

use super::{EventStream, ModelDetails, Provider};
use crate::{
    error::{Error, Result, StreamError, StreamErrorKind},
    event::{Event, FinishReason},
    model::{ModelDeprecation, ReasoningDetails},
    query::ChatQuery,
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Openai;

pub(crate) const ITEM_ID_KEY: &str = "openai_item_id";
pub(crate) const ENCRYPTED_CONTENT_KEY: &str = "openai_encrypted_content";

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

        Ok(self
            .client
            .stream(request)
            .or_else(map_error)
            .map_ok(move |v| stream::iter(map_event(v, is_structured, reasoning_enabled)))
            .try_flatten()
            .chain(future::ready(Ok(Event::Finished(FinishReason::Completed))).into_stream())
            .boxed())
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

/// Create a request for the given model and query details.
///
/// Returns `(request, is_structured, reasoning_enabled)`.
fn create_request(model: &ModelDetails, query: ChatQuery) -> Result<(Request, bool, bool)> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
    } = query;

    // Only use the schema if the very last event is a ChatRequest with one.
    // Transform the schema for OpenAI's strict structured output mode
    // before passing it to the request.
    let text = thread
        .events
        .last()
        .and_then(|e| e.event.as_chat_request())
        .and_then(|req| req.schema.clone())
        .map(|schema| types::TextConfig {
            format: types::TextFormat::JsonSchema {
                schema: Value::Object(transform_schema(schema)),
                description: "Structured output".to_owned(),
                name: "structured_output".to_owned(),
                strict: Some(true),
            },
        });

    let is_structured = text.is_some();
    let parameters = thread.events.config()?.assistant.model.parameters;
    let supports_reasoning = model
        .reasoning
        .is_some_and(|v| !matches!(v, ReasoningDetails::Unsupported));
    let reasoning = match model.custom_reasoning_config(parameters.reasoning) {
        Some(r) => Some(convert_reasoning(r, model.max_output_tokens)),
        // Explicitly disable reasoning for models that support it when the
        // user has turned it off. Sending `null` lets the model use its
        // default (which may include reasoning).
        //
        // For leveled models, use their lowest supported effort. For all
        // others (budgetted), fall back to `minimal` which is universally
        // supported across OpenAI reasoning models.
        None if supports_reasoning => {
            let effort = model
                .reasoning
                .and_then(|r| r.lowest_effort())
                .unwrap_or(ReasoningEffort::Xlow);
            Some(convert_reasoning(
                CustomReasoningConfig {
                    effort,
                    exclude: true,
                },
                model.max_output_tokens,
            ))
        }
        None => None,
    };
    let reasoning_enabled = model
        .custom_reasoning_config(parameters.reasoning)
        .is_some();
    let messages = thread.into_messages(to_system_messages, convert_events(supports_reasoning))?;
    let request = Request {
        model: types::Model::Other(model.id.name.to_string()),
        input: types::Input::List(messages),
        include: reasoning_enabled.then_some(vec![Include::ReasoningEncryptedContent]),
        store: Some(false),
        tool_choice: Some(convert_tool_choice(tool_choice)),
        tools: Some(convert_tools(tools)),
        temperature: parameters.temperature,
        reasoning,
        max_output_tokens: parameters.max_tokens.map(Into::into),
        truncation: Some(types::Truncation::Auto),
        top_p: parameters.top_p,
        text,
        ..Default::default()
    };

    trace!(?request, "Sending request to OpenAI.");

    Ok((request, is_structured, reasoning_enabled))
}

#[expect(clippy::too_many_lines)]
fn map_model(model: ModelResponse) -> Result<ModelDetails> {
    let details = match model.id.as_str() {
        "gpt-5.2-pro" | "gpt-5.2-pro-2025-12-11" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.2 pro".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(false, false, true, true, true)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 9, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.2" | "gpt-5.2-2025-12-11" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.2".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 9, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.1-codex-max" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1-Codex-Max".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.1-codex" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1 Codex".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.1-codex-mini" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1 Codex mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.1" | "gpt-5.1-2025-11-13" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-codex" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5-Codex".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5" | "gpt-5-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 8, 30).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-chat-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 Chat".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 8, 30).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-mini" | "gpt-5-mini-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 8, 30).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-nano" | "gpt-5-nano-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 nano".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 8, 30).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o4-mini" | "o4-mini-2025-04-16" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o4-mini".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o3-mini" | "o3-mini-2025-01-31" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-mini".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1-mini" | "o1-mini-2024-09-12" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1-mini".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(65_536),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: o4-mini",
                Some(NaiveDate::from_ymd_opt(2025, 10, 27).unwrap()),
            )),
            features: vec![],
        },
        "o3" | "o3-2025-04-16" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o3-pro" | "o3-pro-2025-06-10" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-pro".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1" | "o1-2024-12-17" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1-pro" | "o1-pro-2025-03-19" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1-pro".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
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
            features: vec![],
        },
        "gpt-4o" | "gpt-4o-2024-08-06" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4o".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "chatgpt-4o" | "chatgpt-4o-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("ChatGPT-4o".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2023, 10, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.1-chat-latest",
                Some(NaiveDate::from_ymd_opt(2026, 2, 11).unwrap()),
            )),
            features: vec![],
        },
        "gpt-4.1-nano" | "gpt-4.1-nano-2025-04-14" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4.1 nano".to_owned()),
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
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
            features: vec![],
        },
        "o3-deep-research" | "o3-deep-research-2025-06-26" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-deep-research".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o4-mini-deep-research" | "o4-mini-deep-research-2025-06-26" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o4-mini-deep-research".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 6, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        id => {
            warn!(model = id, ?model, "Missing model details.");
            ModelDetails::empty((PROVIDER, id).try_into()?)
        }
    };

    Ok(details)
}

/// Convert an OpenAI [`OpenaiStreamError`] into a [`StreamError`].
///
/// This needs an async function because we read the response body for context.
/// Headers are extracted *before* consuming the body.
async fn map_error(error: OpenaiStreamError) -> std::result::Result<types::Event, StreamError> {
    Err(match error {
        OpenaiStreamError::Stream(error) => StreamError::from(error),
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
        } => vec![Ok(Event::Part {
            event: ConversationEvent::now(if is_structured {
                ChatResponse::structured(Value::String(String::new()))
            } else {
                ChatResponse::message(String::new())
            }),
            index: output_index as usize,
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
        } => vec![Ok(Event::Part {
            event: ConversationEvent::now(ChatResponse::reasoning(String::new())),
            index: output_index as usize,
        })],

        OutputTextDelta {
            delta,
            output_index,
            ..
        } => {
            let response = if is_structured {
                ChatResponse::structured(Value::String(delta))
            } else {
                ChatResponse::message(delta)
            };
            vec![Ok(Event::Part {
                event: ConversationEvent::now(response),
                index: output_index as usize,
            })]
        }

        ReasoningSummaryTextDelta {
            delta,
            output_index,
            ..
        } => vec![Ok(Event::Part {
            event: ConversationEvent::now(ChatResponse::reasoning(delta)),
            index: output_index as usize,
        })],

        OutputItemDone { item, output_index } => {
            let index = output_index as usize;
            let mut events = vec![];
            let metadata = match &item {
                types::OutputItem::FunctionCall(_) => IndexMap::new(),
                types::OutputItem::Message(v) => {
                    let mut map = IndexMap::new();
                    map.insert(ITEM_ID_KEY.to_owned(), v.id.clone().into());
                    map
                }
                types::OutputItem::Reasoning(v) => {
                    let mut map = IndexMap::new();
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

            if let types::OutputItem::FunctionCall(types::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            }) = item
            {
                events.push(Ok(Event::Part {
                    index,
                    event: ConversationEvent::now(ToolCallRequest {
                        id: call_id,
                        name,
                        arguments: if let Ok(arguments) = serde_json::from_str(&arguments) {
                            arguments
                        } else {
                            let mut map = Map::new();
                            map.insert("input".to_owned(), arguments.into());
                            map
                        },
                    }),
                }));
            }

            events.push(Ok(Event::flush_with_metadata(index, metadata)));
            events
        }
        Error { error } => vec![Err(classify_stream_error(error))],
        _ => vec![],
    }
}

/// Classify an OpenAI streaming error event into a [`StreamError`].
///
/// Maps well-known error types (quota, rate-limit, auth, server errors)
/// to the appropriate [`StreamErrorKind`] so the retry and display layers
/// can handle them correctly.
fn classify_stream_error(error: types::response::Error) -> StreamError {
    match error.r#type.as_str() {
        "insufficient_quota" => StreamError::new(
            StreamErrorKind::InsufficientQuota,
            format!(
                "Insufficient API quota. Check your plan and billing details \
                 at https://platform.openai.com/settings/organization/billing. \
                 ({})",
                error.message
            ),
        ),
        "rate_limit_exceeded" => StreamError::rate_limit(None),
        "server_error" | "api_error" => StreamError::transient(error.message),
        _ => StreamError::other(format!(
            "OpenAI error: type={}, code={:?}, message={}, param={:?}",
            error.r#type, error.code, error.message, error.param
        )),
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

/// Transform a JSON schema for OpenAI's strict structured output mode.
///
/// OpenAI's structured outputs require:
/// - `additionalProperties: false` on all objects
/// - All properties listed in `required`
/// - `allOf` is not supported and must be flattened
///
/// Additionally handles:
/// - Unraveling `$ref` that has sibling properties (OpenAI doesn't support
///   `$ref` alongside other keys)
/// - Recursively processing `$defs`/`definitions`, `properties`, `items`, and
///   `anyOf`
/// - Stripping `null` defaults
///
/// Unlike Google, OpenAI supports `$ref`/`$defs` and `const` natively, so those
/// are left in place when standalone.
///
/// See: <https://platform.openai.com/docs/guides/structured-outputs>
fn transform_schema(src: Map<String, Value>) -> Map<String, Value> {
    let root = Value::Object(src.clone());
    process_schema(src, &root)
}

/// Core recursive processor for a single schema node.
fn process_schema(mut src: Map<String, Value>, root: &Value) -> Map<String, Value> {
    // Recursively process $defs/definitions in place.
    for key in ["$defs", "definitions"] {
        if let Some(Value::Object(defs)) = src.remove(key) {
            let processed: Map<String, Value> = defs
                .into_iter()
                .map(|(k, v)| (k, resolve_and_process(v, root)))
                .collect();
            src.insert(key.into(), Value::Object(processed));
        }
    }

    // Force `additionalProperties: false` on all objects.
    // The docs require this for strict mode.
    if src.get("type").and_then(Value::as_str) == Some("object") {
        src.insert("additionalProperties".into(), Value::Bool(false));
    }

    // Force all properties into `required` (strict mode requirement).
    if let Some(Value::Object(props)) = src.get("properties") {
        let keys: Vec<Value> = props.keys().map(|k| Value::String(k.clone())).collect();
        src.insert("required".into(), Value::Array(keys));
    }

    // Recursively process object properties.
    if let Some(Value::Object(props)) = src.remove("properties") {
        let processed: Map<String, Value> = props
            .into_iter()
            .map(|(k, v)| (k, resolve_and_process(v, root)))
            .collect();
        src.insert("properties".into(), Value::Object(processed));
    }

    // Recursively process array items.
    if let Some(items) = src.remove("items") {
        src.insert("items".into(), resolve_and_process(items, root));
    }

    // Recursively process anyOf variants.
    if let Some(Value::Array(variants)) = src.remove("anyOf") {
        src.insert(
            "anyOf".into(),
            Value::Array(
                variants
                    .into_iter()
                    .map(|v| resolve_and_process(v, root))
                    .collect(),
            ),
        );
    }

    // Flatten `allOf` — not supported by OpenAI.
    // Merge all entries into the parent schema; later entries yield to
    // earlier ones (and to keys already present on the parent).
    if let Some(Value::Array(entries)) = src.remove("allOf") {
        for entry in entries {
            if let Value::Object(entry_map) = resolve_and_process(entry, root) {
                for (k, v) in entry_map {
                    src.entry(k).or_insert(v);
                }
            }
        }
    }

    // Strip `null` defaults (no meaningful distinction for strict mode).
    if src.get("default") == Some(&Value::Null) {
        src.remove("default");
    }

    // Unravel `$ref` when it has sibling properties.
    // OpenAI supports standalone `$ref` but not alongside other keys.
    if src.contains_key("$ref")
        && src.len() > 1
        && let Some(Value::String(ref_path)) = src.remove("$ref")
    {
        if let Some(resolved) = resolve_ref(&ref_path, root) {
            // Current schema properties take priority over the
            // resolved definition's.
            let mut merged = resolved;
            for (k, v) in src {
                merged.insert(k, v);
            }
            return process_schema(merged, root);
        }
        // Failed to resolve — put it back.
        src.insert("$ref".into(), Value::String(ref_path));
    }

    src
}

/// Recursively process a value that may be a schema object.
fn resolve_and_process(value: Value, root: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(process_schema(map, root)),
        other => other,
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

            if strict && !cfg.required {
                make_config_nullable(&mut cfg);
            }

            let mut schema = cfg.to_json_schema();

            // If `strict` mode is enabled, we have to adhere to the following
            // rules:
            //
            // - `additionalProperties` must be set to `false` for each object
            // in the `parameters`.
            // - All fields in `properties` must be marked as `required`.
            //
            // See: <https://platform.openai.com/docs/guides/function-calling#strict-mode>
            if strict {
                enforce_strict_object_structure(&mut schema);
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

/// Recursively sets `additionalProperties: false` and ensures nested objects
/// have all their properties marked as required.
fn enforce_strict_object_structure(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            // If it is an object, enforce strictness
            if map.get("type").and_then(|t| t.as_str()) == Some("object") {
                map.insert("additionalProperties".to_owned(), false.into());

                // Nested objects must have ALL properties required
                if let Some(Value::Object(props)) = map.get("properties")
                    && !map.contains_key("required")
                {
                    let keys: Vec<Value> = props.keys().map(|k| k.clone().into()).collect();
                    map.insert("required".to_owned(), Value::Array(keys));
                }
            }

            // Recurse into children
            for (key, value) in map.iter_mut() {
                if key == "properties" || key == "items" || key == "anyOf" {
                    enforce_strict_object_structure(value);
                }
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(enforce_strict_object_structure),
        _ => {}
    }
}

/// Injects nullability into the JSON schema.
fn make_config_nullable(cfg: &mut ToolParameterConfig) {
    match &mut cfg.kind {
        OneOrManyTypes::One(t) if t != "null" => {
            cfg.kind = OneOrManyTypes::Many(vec![t.clone(), "null".to_owned()]);
        }
        OneOrManyTypes::Many(types) if !types.iter().any(|t| t == "null") => {
            types.push("null".to_owned());
        }
        _ => {}
    }
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
            description: tool.description,
            parameters: parameters_with_strict_mode(tool.parameters, true).into(),
        })
        .collect()
}

fn convert_reasoning(
    reasoning: CustomReasoningConfig,
    max_tokens: Option<u32>,
) -> types::ReasoningConfig {
    types::ReasoningConfig {
        summary: if reasoning.exclude {
            None
        } else {
            Some(SummaryConfig::Auto)
        },
        effort: match reasoning
            .effort
            .abs_to_rel(max_tokens)
            .unwrap_or(ReasoningEffort::Auto)
        {
            ReasoningEffort::None => Some(types::ReasoningEffort::None),
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

fn to_system_messages(parts: Vec<String>) -> ListItem {
    ListItem(types::InputListItem::Message(types::InputMessage {
        role: types::Role::System,
        content: types::ContentInput::List(
            parts
                .into_iter()
                .map(|text| types::ContentItem::Text { text })
                .collect(),
        ),
    }))
}

fn convert_events(
    supports_reasoning: bool,
) -> impl Fn(ConversationStream) -> Vec<types::InputListItem> {
    move |events| {
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
                        })]
                    }
                    EventKind::ChatResponse(response) => {
                        let id = metadata
                            .remove(ITEM_ID_KEY)
                            .and_then(|v| v.as_str().map(str::to_owned));

                        let encrypted_content = metadata
                            .remove(ENCRYPTED_CONTENT_KEY)
                            .and_then(|v| v.as_str().map(str::to_owned));

                        match response {
                            ChatResponse::Reasoning { reasoning } => {
                                if supports_reasoning && let Some(id) = id {
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
                                    // Unsupported reasoning content - wrap in XML tags
                                    vec![types::InputListItem::Message(types::InputMessage {
                                        role: types::Role::Assistant,
                                        content: types::ContentInput::Text(format!(
                                            "<think>\n{reasoning}\n</think>\n\n",
                                        )),
                                    })]
                                }
                            }
                            ChatResponse::Message { message } => {
                                if let Some(id) = id {
                                    vec![types::InputListItem::Item(
                                        types::InputItem::OutputMessage(types::OutputMessage {
                                            id,
                                            role: types::Role::Assistant,
                                            content: vec![types::OutputContent::Text {
                                                text: message,
                                                annotations: vec![],
                                            }],
                                            status: types::MessageStatus::Completed,
                                        }),
                                    )]
                                } else {
                                    vec![types::InputListItem::Message(types::InputMessage {
                                        role: types::Role::Assistant,
                                        content: types::ContentInput::Text(message),
                                    })]
                                }
                            }
                            ChatResponse::Structured { data } => {
                                vec![types::InputListItem::Message(types::InputMessage {
                                    role: types::Role::Assistant,
                                    content: types::ContentInput::Text(data.to_string()),
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
}

impl From<types::response::Error> for Error {
    fn from(error: types::response::Error) -> Self {
        Self::OpenaiResponse(error)
    }
}

#[cfg(test)]
#[path = "openai_tests.rs"]
mod tests;
