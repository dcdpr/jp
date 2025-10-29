use std::env;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use indexmap::IndexMap;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    conversation::tool::ToolParameterConfig,
    model::{
        id::{Name, ProviderId},
        parameters::{CustomReasoningConfig, ParametersConfig, ReasoningEffort},
    },
    providers::llm::openai::OpenaiConfig,
};
use jp_conversation::{
    AssistantMessage, MessagePair, UserMessage,
    thread::{Document, Documents, Thread},
};
use openai_responses::{
    Client, CreateError, StreamError,
    types::{self, Include, Request, SummaryConfig},
};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Map, Value};
use time::{OffsetDateTime, macros::date};
use tracing::{debug, trace, warn};

use super::{
    Delta, Event, EventStream, ModelDetails, Provider, ReasoningDetails, Reply, StreamEvent,
};
use crate::{
    error::{Error, Result},
    provider::ModelDeprecation,
    query::ChatQuery,
    stream::{accumulator::Accumulator, event::StreamEndReason},
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Openai;

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

    async fn chat_completion(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<Reply> {
        let client = self.client.clone();
        let request = create_request(model, parameters, query)?;
        client
            .create(request)
            .await?
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
        let request = create_request(model, parameters, query)?;
        let stream = Box::pin(try_stream! {
            let mut accumulator = Accumulator::new(200);
            let stream = client
                .stream(request)
                .or_else(handle_error);

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                for event in map_event(event?, &mut accumulator)? {
                    yield event;
                }
            }
        });

        Ok(stream)
    }
}

fn create_request(
    model: &ModelDetails,
    parameters: &ParametersConfig,
    query: ChatQuery,
) -> Result<Request> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let reasoning_support = model.reasoning;
    let supports_reasoning =
        reasoning_support.is_some_and(|v| matches!(v, ReasoningDetails::Supported { .. }));
    let reasoning = model.custom_reasoning_config(parameters.reasoning);

    let request = Request {
        model: types::Model::Other(model.id.name.to_string()),
        input: convert_thread(thread, supports_reasoning)?,
        include: supports_reasoning.then_some(vec![Include::ReasoningEncryptedContent]),
        store: Some(false),
        tool_choice: Some(convert_tool_choice(tool_choice)),
        tools: Some(convert_tools(tools, tool_call_strict_mode)),
        temperature: parameters.temperature,
        reasoning: reasoning.map(|r| convert_reasoning(r, model.max_output_tokens)),
        max_output_tokens: parameters.max_tokens.map(Into::into),
        truncation: Some(types::Truncation::Auto),
        top_p: parameters.top_p,
        ..Default::default()
    };

    trace!(?request, "Sending request to OpenAI.");

    Ok(request)
}

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
pub(crate) struct ModelListResponse {
    object: String,
    pub data: Vec<ModelResponse>,
}

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
pub(crate) struct ModelResponse {
    pub id: String,
    object: String,
    #[serde(with = "time::serde::timestamp")]
    created: OffsetDateTime,
    owned_by: String,
}

#[expect(clippy::too_many_lines)]
fn map_model(model: ModelResponse) -> Result<ModelDetails> {
    let details = match model.id.as_str() {
        "o4-mini" | "o4-mini-2025-04-16" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o4-mini".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o3-mini" | "o3-mini-2025-01-31" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-mini".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1-mini" | "o1-mini-2024-09-12" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1-mini".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(65_536),
            reasoning: Some(ReasoningDetails::supported(0, None)),
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
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o3-pro" | "o3-pro-2025-06-10" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-pro".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1" | "o1-2024-12-17" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o1-pro" | "o1-pro-2025-03-19" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o1-pro".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
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
            deprecated: Some(ModelDeprecation::Active),
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
        "gpt-5-nano" | "gpt-5-nano-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 nano".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 8 - 30)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-mini" | "gpt-5-mini-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 8 - 30)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5" | "gpt-5-2025-08-07" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 8 - 30)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-5-chat-latest" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("GPT-5 Chat".to_owned()),
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 8 - 30)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-oss-120b" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("gpt-oss-120b".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(131_072),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gpt-oss-20b" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("gpt-oss-20b".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(131_072),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o3-deep-research" | "o3-deep-research-2025-06-26" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o3-deep-research".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "o4-mini-deep-research" | "o4-mini-deep-research-2025-06-26" => ModelDetails {
            id: (PROVIDER, model.id).try_into()?,
            display_name: Some("o4-mini-deep-research".to_owned()),
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
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

async fn handle_error(error: StreamError) -> std::result::Result<types::Event, Error> {
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

fn map_response(response: types::Response) -> Result<Vec<Event>> {
    response
        .output
        .into_iter()
        .filter_map(|item| Delta::from(item).into())
        .collect::<Result<Vec<_>>>()
}

fn map_event(event: types::Event, accumulator: &mut Accumulator) -> Result<Vec<StreamEvent>> {
    use types::Event;

    let delta: Delta = match event {
        Event::OutputTextDelta { delta, .. } => Delta::content(delta),
        Event::OutputItemAdded { item, .. } | Event::OutputItemDone { item, .. }
            if matches!(item, types::OutputItem::FunctionCall(_)) =>
        {
            item.into()
        }
        Event::FunctionCallArgumentsDelta { delta, .. } => Delta::tool_call("", "", delta),
        Event::FunctionCallArgumentsDone { .. } => Delta::tool_call_finished(),
        Event::ReasoningSummaryTextDelta { delta, .. } => Delta::reasoning(delta),
        Event::OutputItemDone {
            item: types::OutputItem::Reasoning(reasoning),
            ..
        } => {
            return match serde_json::to_value(reasoning) {
                Ok(value) => Ok(vec![StreamEvent::Metadata("reasoning".to_owned(), value)]),
                Err(error) => Err(error.into()),
            };
        }
        Event::OutputItemDone { .. } => return accumulator.drain(),
        Event::ResponseIncomplete {
            response:
                types::Response {
                    incomplete_details: Some(details),
                    ..
                },
        } => match details.reason.as_str() {
            "max_tokens" => return Ok(vec![StreamEvent::EndOfStream(StreamEndReason::MaxTokens)]),
            reason => {
                return Ok(vec![StreamEvent::EndOfStream(StreamEndReason::Other(
                    reason.to_owned(),
                ))]);
            }
        },
        Event::ResponseCompleted { .. } => {
            return Ok(vec![StreamEvent::EndOfStream(StreamEndReason::Completed)]);
        }
        _ => {
            trace!(?event, "Ignoring Openai event");
            return Ok(vec![]);
        }
    };

    delta.into_stream_events(accumulator)
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

    let mut properties = parameters
        .into_iter()
        .map(|(k, v)| (k, v.to_json_schema()))
        .collect::<Map<_, _>>();

    // If `strict` mode is enabled, we have to adhere to the
    // following rules:
    //
    // - `additionalProperties` must be set to `false` for each
    // object in the `parameters`.
    // - All fields in `properties` must be marked as `required`.
    //
    // See: <https://platform.openai.com/docs/guides/function-calling#strict-mode>
    if strict {
        properties.iter_mut().for_each(|(_, v)| {
            let current = match v["type"].take() {
                Value::String(s) if s != "null" => vec![s.into(), "null".into()],
                v @ Value::String(_) => vec![v],
                Value::Array(v) => std::iter::once("null".into()).chain(v).collect(),
                _ => vec![],
            };

            v["type"] = Value::Array(current);
        });
    }

    Map::from_iter([
        ("type".to_owned(), "object".into()),
        ("properties".to_owned(), properties.into()),
        ("additionalProperties".to_owned(), (!strict).into()),
        ("required".to_owned(), required.into()),
    ])
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
            Some(SummaryConfig::Detailed)
        },
        effort: match reasoning.effort.abs_to_rel(max_tokens) {
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

fn convert_thread(thread: Thread, supports_reasoning: bool) -> Result<types::Input> {
    Inputs::try_from((thread, supports_reasoning)).map(|v| types::Input::List(v.0))
}

struct Inputs(Vec<types::InputListItem>);

impl TryFrom<(Thread, bool)> for Inputs {
    type Error = Error;

    #[expect(clippy::too_many_lines)]
    fn try_from((thread, supports_reasoning): (Thread, bool)) -> Result<Self> {
        let Thread {
            system_prompt,
            instructions,
            attachments,
            mut history,
            message,
        } = thread;

        // If the last history message is a tool call response, we need to go
        // one more back in history, to avoid disjointing tool call requests and
        // their responses.
        let mut history_after_instructions = vec![];
        while let Some(message) = history.pop() {
            let tool_call_results = matches!(message.message, UserMessage::ToolCallResults(_));
            history_after_instructions.insert(0, message);

            if !tool_call_results {
                break;
            }
        }

        let mut items = vec![];
        let history = history
            .into_iter()
            .flat_map(|v| message_pair_to_messages(v, supports_reasoning))
            .collect::<Vec<_>>();

        // System message first, if any.
        if let Some(system_prompt) = system_prompt {
            items.push(types::InputItem::InputMessage(types::APIInputMessage {
                role: types::Role::System,
                content: types::ContentInput::Text(system_prompt),
                status: None,
            }));
        }

        // Historical messages second, these are static.
        items.extend(history);

        // Group multiple contents blocks into a single message.
        let mut content = vec![];

        if !instructions.is_empty() {
            content.push(
                "Before we continue, here are some contextual details that will help you generate \
                 a better response."
                    .to_string(),
            );
        }

        // Then instructions in XML tags.
        for instruction in &instructions {
            content.push(instruction.try_to_xml()?);
        }

        // Then large list of attachments, formatted as XML.
        if !attachments.is_empty() {
            let documents: Documents = attachments
                .into_iter()
                .enumerate()
                .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            content.push(documents.try_to_xml()?);
        }

        // Attach all data, and add a "fake" acknowledgement by the assistant.
        //
        // See `provider::openrouter` for more information.
        if !content.is_empty() {
            items.push(types::InputItem::InputMessage(types::APIInputMessage {
                role: types::Role::User,
                content: types::ContentInput::List(
                    content
                        .into_iter()
                        .map(|text| types::ContentItem::Text { text })
                        .collect(),
                ),
                status: None,
            }));
        }

        if items.last().is_some_and(|m| match m {
            types::InputItem::InputMessage(message) => matches!(message.role, types::Role::User),
            _ => false,
        }) {
            items.push(types::InputItem::InputMessage(types::APIInputMessage {
                role: types::Role::Assistant,
                content: types::ContentInput::Text(
                    "Thank you for those details, I'll use them to inform my next response."
                        .to_string(),
                ),
                status: None,
            }));
        }

        items.extend(
            history_after_instructions
                .into_iter()
                .flat_map(|v| message_pair_to_messages(v, supports_reasoning)),
        );

        // User query
        match message {
            UserMessage::Query(text) => {
                items.push(types::InputItem::InputMessage(types::APIInputMessage {
                    role: types::Role::User,
                    content: types::ContentInput::Text(text),
                    status: None,
                }));
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| {
                    types::InputItem::FunctionCallOutput(types::FunctionCallOutput {
                        call_id: result.id,
                        output: result.content,
                        id: None,
                        status: None,
                    })
                }));
            }
        }

        Ok(Self(
            items.into_iter().map(types::InputListItem::Item).collect(),
        ))
    }
}

fn message_pair_to_messages(msg: MessagePair, reasoning: bool) -> Vec<types::InputItem> {
    let (user, assistant) = msg.split();

    user_message_to_messages(user)
        .into_iter()
        .chain(assistant_message_to_messages(assistant, reasoning))
        .collect()
}

fn user_message_to_messages(user: UserMessage) -> Vec<types::InputItem> {
    match user {
        UserMessage::Query(query) if !query.is_empty() => {
            vec![types::InputItem::InputMessage(types::APIInputMessage {
                role: types::Role::User,
                content: types::ContentInput::Text(query),
                status: None,
            })]
        }
        UserMessage::Query(_) => vec![],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| {
                types::InputItem::FunctionCallOutput(types::FunctionCallOutput {
                    call_id: result.id,
                    output: result.content,
                    id: None,
                    status: None,
                })
            })
            .collect(),
    }
}

fn assistant_message_to_messages(
    assistant: AssistantMessage,
    supports_reasoning: bool,
) -> Vec<types::InputItem> {
    let AssistantMessage {
        provider,
        metadata,
        reasoning,
        content,
        tool_calls,
    } = assistant;

    let mut items = vec![];
    if supports_reasoning
        && provider == PROVIDER
        && let Some(value) = metadata.get("reasoning").cloned()
    {
        match serde_json::from_value::<types::Reasoning>(value) {
            // If we don't have encrypted content, it means the initial request
            // was made without the `reasoning.encrypted_content` include. Since
            // we don't enable persistent sessions, we can't return the
            // reasoning data without the encrypted content section, or the
            // OpenAI API will return an error.
            //
            // This should normally never happen, since we always ask for this
            // data to be included in the OpenAI responses.
            Ok(reasoning) if reasoning.encrypted_content.is_none() => {
                debug!(?reasoning, "Reasoning missing encrypted content. Ignoring.");
            }

            Ok(reasoning) => items.push(types::InputItem::Reasoning(reasoning)),
            Err(error) => warn!(?error, "Failed to parse OpenAI reasoning data. Ignoring."),
        }
    } else if let Some(reasoning) = reasoning {
        items.push(types::InputItem::InputMessage(types::APIInputMessage {
            role: types::Role::Assistant,
            content: types::ContentInput::Text(format!("<think>\n{reasoning}\n</think>\n\n")),
            status: None,
        }));
    }

    if let Some(text) = content {
        items.push(types::InputItem::InputMessage(types::APIInputMessage {
            role: types::Role::Assistant,
            content: types::ContentInput::Text(text),
            status: None,
        }));
    }

    for tool_call in tool_calls {
        items.push(types::InputItem::FunctionCall(types::FunctionCall {
            call_id: tool_call.id,
            name: tool_call.name,
            arguments: Value::Object(tool_call.arguments).to_string(),
            status: None,
            id: None,
        }));
    }

    items
}

impl From<types::OutputItem> for Delta {
    fn from(item: types::OutputItem) -> Self {
        match item {
            types::OutputItem::Message(message) => Delta::content(
                message
                    .content
                    .into_iter()
                    .filter_map(|item| match item {
                        types::OutputContent::Text { text, .. } => Some(text),
                        types::OutputContent::Refusal { .. } => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
            types::OutputItem::Reasoning(reasoning) => Delta::reasoning(
                reasoning
                    .summary
                    .into_iter()
                    .map(|item| match item {
                        types::ReasoningSummary::Text { text, .. } => text,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
            types::OutputItem::FunctionCall(call) => {
                Delta::tool_call(call.call_id, call.name, call.arguments)
            }
            _ => Delta::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jp_config::providers::llm::LlmProviderConfig;
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr() -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new("https://api.openai.com", fixtures)
    }

    #[test(tokio::test)]
    async fn test_openai_model_details() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().openai;
        let name: Name = "o4-mini".parse().unwrap();

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

                Openai::try_from(&config)
                    .unwrap()
                    .model_details(&name)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_openai_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().openai;
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

                Openai::try_from(&config)
                    .unwrap()
                    .models()
                    .await
                    .map(|mut v| {
                        v.truncate(10);
                        v
                    })
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_openai_chat_completion() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().openai;
        let model_id = "openai/o4-mini".parse().unwrap();
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

                Openai::try_from(&config)
                    .unwrap()
                    .chat_completion(&model, &ParametersConfig::default(), query)
                    .await
            },
        )
        .await
    }
}
