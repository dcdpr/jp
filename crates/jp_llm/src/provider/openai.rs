use std::env;

use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use jp_config::llm;
use jp_conversation::{
    model::{ProviderId, Reasoning, ReasoningEffort},
    thread::{Document, Documents, Thread},
    AssistantMessage, MessagePair, Model, UserMessage,
};
use jp_mcp::tool;
use jp_query::query::ChatQuery;
use openai_responses::{
    types::{self, Include, Request, SummaryConfig},
    Client, CreateError, StreamError,
};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::Value;
use time::{macros::date, OffsetDateTime};
use tracing::{debug, trace, warn};

use super::{handle_delta, Delta, Event, EventStream, ModelDetails, Provider, StreamEvent};
use crate::{
    error::{Error, Result},
    provider::AccumulationState,
};

#[derive(Debug, Clone)]
pub struct Openai {
    reqwest_client: reqwest::Client,
    client: Client,
    base_url: String,
}

impl Openai {
    async fn create_request(&self, model: &Model, query: ChatQuery) -> Result<Request> {
        let ChatQuery {
            thread,
            tools,
            tool_choice,
            tool_call_strict_mode,
        } = query;

        let model_details = self
            .models()
            .await?
            .into_iter()
            .find(|m| m.slug == model.id.slug());

        let supports_reasoning = model_details
            .as_ref()
            .is_some_and(|d| d.reasoning.is_some_and(|v| v));

        let request = Request {
            model: types::Model::Other(model.id.slug().to_owned()),
            input: convert_thread(thread, supports_reasoning)?,
            include: supports_reasoning.then_some(vec![Include::ReasoningEncryptedContent]),
            store: Some(false),
            tool_choice: Some(convert_tool_choice(tool_choice)),
            tools: Some(convert_tools(tools, tool_call_strict_mode)),
            temperature: model.parameters.temperature,
            reasoning: model
                .parameters
                .reasoning
                .map(|r| convert_reasoning(r, model_details.and_then(|d| d.max_output_tokens))),
            max_output_tokens: model.parameters.max_tokens.map(Into::into),
            truncation: Some(types::Truncation::Auto),
            top_p: model.parameters.top_p,
            ..Default::default()
        };

        Ok(request)
    }
}

#[async_trait]
impl Provider for Openai {
    async fn models(&self) -> Result<Vec<ModelDetails>> {
        Ok(self
            .reqwest_client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<ModelListResponse>()
            .await?
            .data
            .into_iter()
            .map(map_model)
            .collect())
    }

    async fn chat_completion(&self, model: &Model, query: ChatQuery) -> Result<Vec<Event>> {
        let client = self.client.clone();
        let request = self.create_request(model, query).await?;
        client
            .create(request)
            .await?
            .map_err(Into::into)
            .and_then(map_response)
    }

    async fn chat_completion_stream(&self, model: &Model, query: ChatQuery) -> Result<EventStream> {
        let client = self.client.clone();
        let request = self.create_request(model, query).await?;
        let stream = Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            let stream = client
                .stream(request)
                .or_else(handle_error);

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                if let Some(event) = map_event(event?, &mut current_state) {
                    yield event;
                }
            }
        });

        Ok(stream)
    }
}

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
struct ModelListResponse {
    object: String,
    data: Vec<ModelResponse>,
}

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
struct ModelResponse {
    id: String,
    object: String,
    #[serde(with = "time::serde::timestamp")]
    created: OffsetDateTime,
    owned_by: String,
}

#[expect(clippy::too_many_lines)]
fn map_model(model: ModelResponse) -> ModelDetails {
    match model.id.as_str() {
        "o4-mini" | "o4-mini-2025-04-16" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(true),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
        },
        "o3-mini" | "o3-mini-2025-01-31" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(true),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
        },
        "o1-mini" | "o1-mini-2024-09-12" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(128_000),
            max_output_tokens: Some(65_536),
            reasoning: Some(true),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
        },
        "o3" | "o3-2025-04-16" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(true),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
        },
        "o1" | "o1-2024-12-17" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(true),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
        },
        "o1-pro" | "o1-pro-2025-03-19" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(100_000),
            reasoning: Some(true),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
        },
        "gpt-4.1" | "gpt-4.1-2025-04-14" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
        },
        "gpt-4o" | "gpt-4o-2024-08-06" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
        },
        "chatgpt-4o" | "chatgpt-4o-latest" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
        },
        "gpt-4.1-nano" | "gpt-4.1-nano-2025-04-14" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
        },
        "gpt-4o-mini" | "gpt-4o-mini-2024-07-18" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(128_000),
            max_output_tokens: Some(16_384),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2023 - 10 - 1)),
        },
        "gpt-4.1-mini" | "gpt-4.1-mini-2025-04-14" => ModelDetails {
            provider: ProviderId::Openai,
            slug: model.id,
            context_window: Some(1_047_576),
            max_output_tokens: Some(32_768),
            reasoning: Some(false),
            knowledge_cutoff: Some(date!(2024 - 6 - 1)),
        },
        id => {
            warn!(model = id, ?model, "Missing model details.");

            ModelDetails {
                provider: ProviderId::Openai,
                slug: model.id,
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            }
        }
    }
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

fn map_event(event: types::Event, state: &mut AccumulationState) -> Option<Result<StreamEvent>> {
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
                Ok(value) => Some(Ok(StreamEvent::Metadata("reasoning".to_owned(), value))),
                Err(error) => Some(Err(error.into())),
            }
        }
        _ => {
            trace!(?event, "Ignoring Openai event");
            return None;
        }
    };

    handle_delta(delta, state).transpose()
}

impl TryFrom<&llm::provider::openai::Config> for Openai {
    type Error = Error;

    fn try_from(config: &llm::provider::openai::Config) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        let reqwest_client = reqwest::Client::builder()
            .default_headers(HeaderMap::from_iter([(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}"))
                    .map_err(|_| CreateError::InvalidApiKey)?,
            )]))
            .build()?;

        let client = Client::new(&api_key)?.with_base_url(config.base_url.clone());

        Ok(Openai {
            reqwest_client,
            client,
            base_url: config.base_url.clone(),
        })
    }
}

fn convert_tool_choice(choice: tool::ToolChoice) -> types::ToolChoice {
    match choice {
        tool::ToolChoice::Auto => types::ToolChoice::Auto,
        tool::ToolChoice::None => types::ToolChoice::None,
        tool::ToolChoice::Required => types::ToolChoice::Required,
        tool::ToolChoice::Function(name) => types::ToolChoice::Function(name),
    }
}

fn convert_tools(tools: Vec<jp_mcp::Tool>, strict: bool) -> Vec<types::Tool> {
    tools
        .into_iter()
        .map(|tool| types::Tool::Function {
            name: tool.name.into(),
            parameters: Value::Object(tool.input_schema.as_ref().clone()),
            strict,
            description: tool.description.map(|v| v.to_string()),
        })
        .collect()
}

fn convert_reasoning(reasoning: Reasoning, max_tokens: Option<u32>) -> types::ReasoningConfig {
    types::ReasoningConfig {
        summary: if reasoning.exclude {
            None
        } else {
            Some(SummaryConfig::Detailed)
        },
        effort: match reasoning.effort.abs_to_rel(max_tokens) {
            ReasoningEffort::High => Some(types::ReasoningEffort::High),
            ReasoningEffort::Medium => Some(types::ReasoningEffort::Medium),
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
        metadata,
        reasoning: _,
        content,
        tool_calls,
    } = assistant;

    let mut items = vec![];
    if supports_reasoning && let Some(value) = metadata.get("reasoning").cloned() {
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
            arguments: tool_call.arguments.to_string(),
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

    use jp_conversation::ModelId;
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr() -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new("https://api.openai.com", fixtures)
    }

    #[test(tokio::test)]
    async fn test_openai_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.openai;
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
                        v.truncate(2);
                        v
                    })
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_openai_chat_completion() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.openai;
        let model: ModelId = "openai/o4-mini".parse().unwrap();
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
                    .chat_completion(&model.into(), query)
                    .await
            },
        )
        .await
    }
}
