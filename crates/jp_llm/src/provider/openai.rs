use std::env;

use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use jp_config::llm;
use jp_conversation::{
    model,
    thread::{Document, Documents, Thinking, Thread},
    AssistantMessage, MessagePair, Model, UserMessage,
};
use jp_query::query::ChatQuery;
use openai_responses::{
    types::{self, ReasoningEffort, Request},
    Client, StreamError,
};
use serde_json::Value;
use tracing::trace;

use super::{handle_delta, Delta, Event, EventStream, Provider, StreamEvent};
use crate::{
    error::{Error, Result},
    provider::AccumulationState,
};

#[derive(Debug, Clone)]
pub struct Openai {
    client: Client,
}

#[async_trait]
impl Provider for Openai {
    async fn chat_completion(&self, model: &Model, query: ChatQuery) -> Result<Vec<Event>> {
        let client = self.client.clone();
        let request = create_request(model, query)?;
        client
            .create(request)
            .await?
            .map_err(Into::into)
            .and_then(map_response)
    }

    fn chat_completion_stream(&self, model: &Model, query: ChatQuery) -> Result<EventStream> {
        let client = self.client.clone();
        let request = create_request(model, query)?;
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
        _ => {
            trace!(?event, "Ignoring Openai event");
            return None;
        }
    };

    handle_delta(delta, state).transpose()
}

fn create_request(model: &Model, query: ChatQuery) -> Result<Request> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let request = Request {
        model: types::Model::Other(model.slug.clone()),
        input: convert_thread(thread)?,
        store: Some(false),
        tool_choice: Some(convert_tool_choice(tool_choice)),
        tools: Some(convert_tools(tools, tool_call_strict_mode)),
        temperature: model.temperature,
        reasoning: model.reasoning.map(convert_reasoning),
        max_output_tokens: model.max_tokens.map(Into::into),
        truncation: Some(types::Truncation::Auto),
        top_p: model
            .additional_parameters
            .get("top_p")
            .and_then(Value::as_f64)
            .map(
                #[expect(clippy::cast_possible_truncation)]
                |v| v as f32,
            ),
        ..Default::default()
    };

    Ok(request)
}

impl TryFrom<&llm::provider::openai::Config> for Openai {
    type Error = Error;

    fn try_from(config: &llm::provider::openai::Config) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        Ok(Openai {
            client: Client::new(&api_key)?,
        })
    }
}

fn convert_tool_choice(choice: llm::ToolChoice) -> types::ToolChoice {
    match choice {
        llm::ToolChoice::Auto => types::ToolChoice::Auto,
        llm::ToolChoice::None => types::ToolChoice::None,
        llm::ToolChoice::Required => types::ToolChoice::Required,
        llm::ToolChoice::Function(name) => types::ToolChoice::Function(name),
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

fn convert_reasoning(reasoning: model::Reasoning) -> types::ReasoningConfig {
    types::ReasoningConfig {
        // TODO: needs "organization ID-check verification" on OpenAI platform.
        // summary: Some(SummaryConfig::Auto),
        summary: None,
        effort: Some(match reasoning.effort {
            model::ReasoningEffort::High => ReasoningEffort::High,
            model::ReasoningEffort::Medium => ReasoningEffort::Medium,
            model::ReasoningEffort::Low => ReasoningEffort::Low,
        }),
    }
}

fn convert_thread(thread: Thread) -> Result<types::Input> {
    Inputs::try_from(thread).map(|v| types::Input::List(v.0))
}

struct Inputs(Vec<types::InputListItem>);

impl TryFrom<Thread> for Inputs {
    type Error = Error;

    #[expect(clippy::too_many_lines)]
    fn try_from(thread: Thread) -> Result<Self> {
        let Thread {
            system_prompt,
            instructions,
            attachments,
            mut history,
            reasoning,
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
            .flat_map(message_pair_to_messages)
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
                .flat_map(message_pair_to_messages),
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

        // Reasoning message last, in `<thinking>` tags.
        if let Some(content) = reasoning {
            items.push(types::InputItem::InputMessage(types::APIInputMessage {
                role: types::Role::Assistant,
                content: types::ContentInput::Text(Thinking(content).try_to_xml()?),
                status: None,
            }));
        }

        Ok(Self(
            items.into_iter().map(types::InputListItem::Item).collect(),
        ))
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<types::InputItem> {
    let (user, assistant) = msg.split();

    user_message_to_messages(user)
        .into_iter()
        .chain(assistant_message_to_messages(assistant))
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

fn assistant_message_to_messages(assistant: AssistantMessage) -> Vec<types::InputItem> {
    let AssistantMessage {
        content,
        tool_calls,
        ..
    } = assistant;

    let mut items = vec![];
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
