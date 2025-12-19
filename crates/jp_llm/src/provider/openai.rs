use std::env;

use async_trait::async_trait;
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _, future, stream};
use indexmap::{IndexMap, IndexSet};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    conversation::tool::{OneOrManyTypes, ToolParameterConfig, item::ToolParameterItemConfig},
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
    Client, CreateError, StreamError,
    types::{self, Include, Request, SummaryConfig},
};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Map, Value};
use time::{OffsetDateTime, macros::date};
use tracing::{trace, warn};

use super::{EventStream, ModelDetails, Provider};
use crate::{
    error::{Error, Result},
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
        let request = create_request(model, query)?;

        Ok(self
            .client
            .stream(request)
            .or_else(map_error)
            .map_ok(|v| stream::iter(map_event(v)))
            .try_flatten()
            .chain(future::ok(Event::Finished(FinishReason::Completed)).into_stream())
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
    #[serde(rename = "created", with = "time::serde::timestamp")]
    _created: OffsetDateTime,
    #[serde(rename = "owned_by")]
    _owned_by: String,
}

/// Create a request for the given model and query details.
fn create_request(model: &ModelDetails, query: ChatQuery) -> Result<Request> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let parameters = thread.events.config()?.assistant.model.parameters;
    let reasoning = model
        .custom_reasoning_config(parameters.reasoning)
        .map(|r| convert_reasoning(r, model.max_output_tokens));
    let supports_reasoning = model
        .reasoning
        .is_some_and(|v| !matches!(v, ReasoningDetails::Unsupported));
    let messages = thread.into_messages(to_system_messages, convert_events(supports_reasoning))?;

    let request = Request {
        model: types::Model::Other(model.id.name.to_string()),
        input: types::Input::List(messages),
        include: supports_reasoning.then_some(vec![Include::ReasoningEncryptedContent]),
        store: Some(false),
        tool_choice: Some(convert_tool_choice(tool_choice)),
        tools: Some(convert_tools(tools, tool_call_strict_mode)),
        temperature: parameters.temperature,
        reasoning,
        max_output_tokens: parameters.max_tokens.map(Into::into),
        truncation: Some(types::Truncation::Auto),
        top_p: parameters.top_p,
        ..Default::default()
    };

    trace!(?request, "Sending request to OpenAI.");

    Ok(request)
}

#[expect(clippy::too_many_lines)]
fn map_model(model: ModelResponse) -> Result<ModelDetails> {
    let details = match model.id.as_str() {
        "gpt-5.2-pro" | "gpt-5.2-pro-2025-12-11" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.2 pro".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::leveled(false, true, true, true)),
            knowledge_cutoff: Some(date!(2025 - 9 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.2" | "gpt-5.2-2025-12-11" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.2".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2025 - 9 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.1-codex-max" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1-Codex-Max".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.1-codex" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1 Codex".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.1-codex-mini" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1 Codex mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5.1" | "gpt-5.1-2025-11-13" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5.1".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-codex" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5-Codex".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5" | "gpt-5-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 8 - 30)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-chat-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 Chat".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 8 - 30)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-mini" | "gpt-5-mini-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 8 - 30)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-nano" | "gpt-5-nano-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 nano".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 8 - 30)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o4-mini" | "o4-mini-2025-04-16" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o4-mini".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o3-mini" | "o3-mini-2025-01-31" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-mini".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1-mini" | "o1-mini-2024-09-12" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1-mini".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(65_536),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: o4-mini",
                Some(date!(2025 - 10 - 27)),
            )),
            features: vec![],
        },
        "o3" | "o3-2025-04-16" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o3-pro" | "o3-pro-2025-06-10" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-pro".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1" | "o1-2024-12-17" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1-pro" | "o1-pro-2025-03-19" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1-pro".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-4.1" | "gpt-4.1-2025-04-14" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4.1".to_owned()),
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-4o" | "gpt-4o-2024-08-06" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4o".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "chatgpt-4o" | "chatgpt-4o-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("ChatGPT-4o".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gpt-5.1-chat-latest",
                Some(date!(2026 - 02 - 11)),
            )),
            features: vec![],
        },
        "gpt-4.1-nano" | "gpt-4.1-nano-2025-04-14" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4.1 nano".to_owned()),
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-4o-mini" | "gpt-4o-mini-2024-07-18" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4o mini".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-4.1-mini" | "gpt-4.1-mini-2025-04-14" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-4.1 mini".to_owned()),
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-oss-120b" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("gpt-oss-120b".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(131_072),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-oss-20b" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("gpt-oss-20b".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(131_072),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o3-deep-research" | "o3-deep-research-2025-06-26" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-deep-research".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o4-mini-deep-research" | "o4-mini-deep-research-2025-06-26" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o4-mini-deep-research".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
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

/// Convert a [`StreamError`] into an [`Error`].
///
/// This needs an async function because we want to get the response text from
/// the body as contextual information.
async fn map_error(error: StreamError) -> Result<types::Event> {
    Err(match error {
        StreamError::Parsing(error) => error.into(),
        StreamError::Stream(error) => match error {
            reqwest_eventsource::Error::InvalidStatusCode(status_code, response) => {
                Error::OpenaiStatusCode {
                    status_code,
                    response: response.text().await.unwrap_or_default(),
                }
            }
            _ => Error::OpenaiEvent(Box::new(error)),
        },
    })
}

/// Map an Openai [`types::Event`] into one or more [`Event`]s.
fn map_event(event: types::Event) -> Vec<Result<Event>> {
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
            event: ConversationEvent::now(ChatResponse::message(String::new())),
            index: output_index as usize,
        })],

        // See the previous `OutputItemAdded` case for details.
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
        } => vec![Ok(Event::Part {
            event: ConversationEvent::now(ChatResponse::message(delta)),
            index: output_index as usize,
        })],

        ReasoningSummaryTextDelta {
            delta,
            output_index,
            ..
        } => vec![Ok(Event::Part {
            event: ConversationEvent::now(ChatResponse::reasoning(delta)),
            index: output_index as usize,
        })],

        OutputItemDone { item, output_index } => {
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

            match item {
                types::OutputItem::FunctionCall(types::FunctionCall {
                    name,
                    arguments,
                    call_id,
                    ..
                }) => vec![Ok(Event::Part {
                    index: output_index as usize,
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
                })],
                _ => vec![Ok(Event::flush_with_metadata(
                    output_index as usize,
                    metadata,
                ))],
            }
        }
        Error {
            code,
            message,
            param,
        } => vec![Err(types::Error {
            r#type: "stream_error".to_owned(),
            code,
            message,
            param,
        }
        .into())],
        _ => vec![],
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
        let mut item_config = items.clone().into();
        sanitize_parameter(&mut item_config);
        *items = item_config.into();
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

        ToolParameterItemConfig {
            kind,
            default: None,
            description: None,
            enumeration: vec![],
        }
    });

    // Append the flattened values to the items enum
    items_config.enumeration.extend(items);
}

fn convert_tools(tools: Vec<ToolDefinition>, strict: bool) -> Vec<types::Tool> {
    tools
        .into_iter()
        .map(|tool| types::Tool::Function {
            name: tool.name,
            strict,
            description: tool.description,
            parameters: parameters_with_strict_mode(tool.parameters, strict).into(),
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
        effort: match reasoning.effort.abs_to_rel(max_tokens) {
            ReasoningEffort::XHigh => Some(types::ReasoningEffort::XHigh),
            ReasoningEffort::High => Some(types::ReasoningEffort::High),
            ReasoningEffort::Auto | ReasoningEffort::Medium => Some(types::ReasoningEffort::Medium),
            ReasoningEffort::Low => Some(types::ReasoningEffort::Low),
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

// /// Converts a single event into `OpenAI` input items.
// ///
// /// Note: `OpenAI` requires separate items for different content types.
// fn convert_events(
//     events: ConversationStream,
//     supports_reasoning: bool,
// ) -> Vec<types::InputListItem> {
//     events
//         .into_iter()
//         .flat_map(|event| {
//             let ConversationEvent {
//                 kind, mut metadata, ..
//             } = event.event;
//
//             match kind {
//                 EventKind::ChatRequest(request) => {
//                     vec![types::InputListItem::Message(types::InputMessage {
//                         role: types::Role::User,
//                         content: types::ContentInput::Text(request.content),
//                     })]
//                 }
//                 EventKind::ChatResponse(response) => {
//                     if let Some(item) = metadata.remove(ENCODED_PAYLOAD_KEY).and_then(|s| {
//                         Some(if response.is_reasoning() {
//                             if !supports_reasoning {
//                                 return None;
//                             }
//
//                             types::InputItem::Reasoning(
//                                 serde_json::from_value::<types::Reasoning>(s).ok()?,
//                             )
//                         } else {
//                             types::InputItem::OutputMessage(
//                                 serde_json::from_value::<types::OutputMessage>(s).ok()?,
//                             )
//                         })
//                     }) {
//                         vec![types::InputListItem::Item(item)]
//                     } else if response.is_reasoning() {
//                         // Unsupported reasoning content - wrap in XML tags
//                         vec![types::InputListItem::Message(types::InputMessage {
//                             role: types::Role::Assistant,
//                             content: types::ContentInput::Text(format!(
//                                 "<think>\n{}\n</think>\n\n",
//                                 response.content()
//                             )),
//                         })]
//                     } else {
//                         vec![types::InputListItem::Message(types::InputMessage {
//                             role: types::Role::Assistant,
//                             content: types::ContentInput::Text(response.into_content()),
//                         })]
//                     }
//                 }
//                 EventKind::ToolCallRequest(request) => {
//                     let call = metadata
//                         .remove(ENCODED_PAYLOAD_KEY)
//                         .and_then(|s| serde_json::from_value::<types::FunctionCall>(s).ok())
//                         .unwrap_or_else(|| types::FunctionCall {
//                             call_id: String::new(),
//                             name: request.name,
//                             arguments: Value::Object(request.arguments).to_string(),
//                             status: None,
//                             id: (!request.id.is_empty()).then_some(request.id),
//                         });
//
//                     vec![types::InputListItem::Item(types::InputItem::FunctionCall(
//                         call,
//                     ))]
//                 }
//                 EventKind::ToolCallResponse(ToolCallResponse { id, result }) => {
//                     vec![types::InputListItem::Item(
//                         types::InputItem::FunctionCallOutput(types::FunctionCallOutput {
//                             call_id: id,
//                             output: match result {
//                                 Ok(content) | Err(content) => content,
//                             },
//                             id: None,
//                             status: None,
//                         }),
//                     )]
//                 }
//                 _ => vec![],
//             }
//         })
//         .collect()
// }

// impl From<types::OutputItem> for Delta {
//     fn from(item: types::OutputItem) -> Self {
//         match item {
//             types::OutputItem::Message(message) => Delta::content(
//                 message
//                     .content
//                     .into_iter()
//                     .filter_map(|item| match item {
//                         types::OutputContent::Text { text, .. } => Some(text),
//                         types::OutputContent::Refusal { .. } => None,
//                     })
//                     .collect::<Vec<_>>()
//                     .join("\n\n"),
//             ),
//             types::OutputItem::Reasoning(reasoning) => Delta::reasoning(
//                 reasoning
//                     .summary
//                     .into_iter()
//                     .map(|item| match item {
//                         types::ReasoningSummary::Text { text, .. } => text,
//                     })
//                     .collect::<Vec<_>>()
//                     .join("\n\n"),
//             ),
//             types::OutputItem::FunctionCall(call) => {
//                 Delta::tool_call(call.call_id, call.name, call.arguments).finished()
//             }
//             _ => Delta::default(),
//         }
//     }
// }

// #[cfg(test)]
// mod tests {
//     use jp_config::providers::llm::LlmProviderConfig;
//     use jp_test::{Result, fn_name, mock::Vcr};
//     use test_log::test;
//
//     use super::*;
//
//     fn vcr() -> Vcr {
//         Vcr::new("https://api.openai.com", env!("CARGO_MANIFEST_DIR"))
//     }
//
//     #[test(tokio::test)]
//     async fn test_openai_model_details() -> Result {
//         let mut config = LlmProviderConfig::default().openai;
//         let name: Name = "o4-mini".parse().unwrap();
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
//                 Openai::try_from(&config)
//                     .unwrap()
//                     .model_details(&name)
//                     .await
//             },
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_openai_models() -> Result {
//         let mut config = LlmProviderConfig::default().openai;
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
//                 Openai::try_from(&config)
//                     .unwrap()
//                     .models()
//                     .await
//                     .map(|mut v| {
//                         v.truncate(10);
//                         v
//                     })
//             },
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_openai_chat_completion() -> Result {
//         let mut config = LlmProviderConfig::default().openai;
//         let model_id = "openai/o4-mini".parse().unwrap();
//         let model = ModelDetails::empty(model_id);
//         let query = ChatQuery {
//             thread: Thread {
//                 events: ConversationStream::default().with_chat_request("Test message"),
//                 ..Default::default()
//             },
//             ..Default::default()
//         };
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
//                 Openai::try_from(&config)
//                     .unwrap()
//                     .chat_completion(&model, query)
//                     .await
//             },
//         )
//         .await
//     }
// }
