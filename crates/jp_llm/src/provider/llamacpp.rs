use std::mem;

use async_stream::try_stream;
use async_trait::async_trait;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ParametersConfig,
    },
    providers::llm::llamacpp::LlamacppConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{EventKind, ToolCallResponse},
    thread::{Document, Documents, Thread},
};
use openai::{
    Credentials,
    chat::{
        self, ChatCompletionBuilder, ChatCompletionChoiceDelta, ChatCompletionDelta,
        ChatCompletionGeneric, ChatCompletionMessage, ChatCompletionMessageDelta,
        ChatCompletionMessageRole, ToolCallFunction, structured_output::ToolCallFunctionDefinition,
    },
};
use serde_json::Value;
use tracing::{debug, trace};

use super::{
    CompletionChunk, Delta, EventStream, ModelDetails, StreamEvent,
    openai::{ModelListResponse, ModelResponse},
};
use crate::{
    error::{Error, Result},
    provider::{Provider, ReasoningExtractor},
    query::ChatQuery,
    stream::accumulator::Accumulator,
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Llamacpp;

#[derive(Debug, Clone)]
pub struct Llamacpp {
    reqwest_client: reqwest::Client,
    credentials: Credentials,
    base_url: String,
}

impl Llamacpp {
    /// Build request for Llama.cpp API.
    fn build_request(
        &self,
        model: &ModelDetails,
        _parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<ChatCompletionBuilder> {
        let slug = model.id.name.to_string();
        let ChatQuery {
            thread,
            tools,
            tool_choice,
            tool_call_strict_mode,
        } = query;

        let messages = convert_thread(thread)?;
        let tools = convert_tools(tools, tool_call_strict_mode, &tool_choice);
        let tool_choice = convert_tool_choice(&tool_choice);

        trace!(
            slug,
            messages_size = messages.len(),
            tools_size = tools.len(),
            "Built Llamacpp request."
        );

        Ok(ChatCompletionDelta::builder(&slug, messages)
            .credentials(self.credentials.clone())
            .tools(tools)
            .tool_choice(tool_choice))
    }
}

#[async_trait]
impl Provider for Llamacpp {
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
        self.reqwest_client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<ModelListResponse>()
            .await?
            .data
            .iter()
            .map(map_model)
            .collect::<Result<_>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<EventStream> {
        debug!(
            model = %model.id.name,
            "Starting Llamacpp chat completion stream."
        );

        let request = self.build_request(model, parameters, query)?;
        Ok(Box::pin(try_stream!({
            let mut accumulator = Accumulator::new(200);
            let mut reasoning_extractor = ReasoningExtractor::default();

            let stream = request
                .create_stream()
                .await
                .expect("Should not fail to clone");
            tokio::pin!(stream);

            while let Some(delta) = stream.recv().await {
                let Some((delta, finish_reason)) = delta
                    .choices
                    .into_iter()
                    .next()
                    .map(|c| (c.delta, c.finish_reason))
                else {
                    continue;
                };

                reasoning_extractor.handle(delta.content.as_deref().unwrap_or_default());

                if finish_reason.is_some() {
                    reasoning_extractor.finalize();
                }

                for event in map_event(
                    delta,
                    &mut accumulator,
                    &mut reasoning_extractor,
                    finish_reason.as_deref(),
                )? {
                    yield event;
                }
            }
        })))
    }
}

fn map_event(
    event: ChatCompletionMessageDelta,
    accumulator: &mut Accumulator,
    extractor: &mut ReasoningExtractor,
    finish_reason: Option<&str>,
) -> Result<Vec<StreamEvent>> {
    let mut events = vec![];

    for chat::ToolCallDelta { id, function, .. } in event.tool_calls.into_iter().flatten() {
        let (name, arguments) = match function {
            Some(chat::ToolCallFunction { name, arguments }) => (name, arguments),
            None => (String::new(), String::new()),
        };

        let mut delta = Delta::tool_call(id.unwrap_or_default(), name, arguments);

        if finish_reason == Some("function_call") {
            delta.tool_call_finished = true;
        }

        events.extend(delta.into_stream_events(accumulator)?);
    }

    events.extend(map_content(
        accumulator,
        extractor,
        finish_reason.is_some(),
    )?);

    Ok(events)
}

fn map_content(
    accumulator: &mut Accumulator,
    extractor: &mut ReasoningExtractor,
    done: bool,
) -> Result<Vec<StreamEvent>> {
    let mut events = Vec::new();
    if !extractor.reasoning.is_empty() {
        let reasoning = mem::take(&mut extractor.reasoning);
        events.extend(Delta::reasoning(reasoning).into_stream_events(accumulator)?);
    }

    if !extractor.other.is_empty() {
        let content = mem::take(&mut extractor.other);
        events.extend(Delta::content(content).into_stream_events(accumulator)?);
    }

    if done {
        events.extend(accumulator.drain()?);
    }

    Ok(events)
}

fn map_model(model: &ModelResponse) -> Result<ModelDetails> {
    Ok(ModelDetails {
        id: (
            PROVIDER,
            model
                .id
                .rsplit_once('/')
                .map_or(model.id.as_str(), |(_, v)| v),
        )
            .try_into()?,
        display_name: None,
        context_window: None,
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        features: vec![],
    })
}

impl TryFrom<&LlamacppConfig> for Llamacpp {
    type Error = Error;

    fn try_from(config: &LlamacppConfig) -> Result<Self> {
        let reqwest_client = reqwest::Client::builder().build()?;
        let base_url = config.base_url.clone();
        let credentials = Credentials::new("", &base_url);

        Ok(Llamacpp {
            reqwest_client,
            credentials,
            base_url,
        })
    }
}

impl From<ChatCompletionGeneric<ChatCompletionChoiceDelta>> for CompletionChunk {
    fn from(chunk: ChatCompletionGeneric<ChatCompletionChoiceDelta>) -> Self {
        let content = chunk
            .choices
            .first()
            .and_then(|choice| choice.delta.content.as_deref().map(String::from))
            .unwrap_or_default();

        Self::Content(content)
    }
}

/// Convert a list of [`jp_mcp::Tool`] to a list of [`chat::ChatCompletionTool`].
///
/// Additionally, if [`ToolChoice::Function`] is provided, only return the
/// tool(s) that matches the expected name. This is done because Llama.cpp does
/// not support calling a specific tool by name, but it *does* support
/// "required"/forced tool calling, which we can turn into a request to call a
/// specific tool, by limiting the list of tools to the ones that we want to be
/// called.
fn convert_tools(
    tools: Vec<ToolDefinition>,
    strict: bool,
    tool_choice: &ToolChoice,
) -> Vec<chat::ChatCompletionTool> {
    tools
        .into_iter()
        .map(|tool| chat::ChatCompletionTool::Function {
            function: ToolCallFunctionDefinition {
                parameters: Some(tool.to_parameters_map().into()),
                name: tool.name,
                description: tool.description,
                strict: Some(strict),
            },
        })
        .filter(|tool| match tool_choice {
            ToolChoice::Function(req) => matches!(
                tool,
                chat::ChatCompletionTool::Function {
                    function: ToolCallFunctionDefinition { name, .. }
                } if name == req
            ),
            _ => true,
        })
        .collect::<Vec<_>>()
}

fn convert_tool_choice(choice: &ToolChoice) -> chat::ToolChoice {
    match choice {
        ToolChoice::Auto => chat::ToolChoice::mode(chat::ToolChoiceMode::Auto),
        ToolChoice::None => chat::ToolChoice::mode(chat::ToolChoiceMode::None),
        ToolChoice::Required | ToolChoice::Function(_) => {
            chat::ToolChoice::mode(chat::ToolChoiceMode::Required)
        }
    }
}

fn convert_thread(thread: Thread) -> Result<Vec<ChatCompletionMessage>> {
    let Thread {
        system_prompt,
        instructions,
        attachments,
        events,
    } = thread;

    let mut items = vec![];

    // Build system prompt with instructions and attachments
    let mut system_parts = vec![];

    if let Some(system_prompt) = system_prompt {
        system_parts.push(system_prompt);
    }

    if !instructions.is_empty() {
        system_parts.push(
            "Before we continue, here are some contextual details that will help you generate a \
             better response."
                .to_string(),
        );

        for instruction in &instructions {
            system_parts.push(instruction.try_to_xml()?);
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

        system_parts.push(documents.try_to_xml()?);
    }

    // Add system message if we have any system content
    if !system_parts.is_empty() {
        items.push(ChatCompletionMessage {
            role: ChatCompletionMessageRole::System,
            content: Some(system_parts.join("\n\n")),
            ..Default::default()
        });
    }

    let messages = convert_events(events);
    items.extend(messages);

    Ok(items)
}

fn convert_events(events: ConversationStream) -> Vec<ChatCompletionMessage> {
    events
        .into_iter()
        .filter_map(|event| match event.into_kind() {
            EventKind::ChatRequest(request) => Some(ChatCompletionMessage {
                role: ChatCompletionMessageRole::User,
                content: Some(request.content),
                ..Default::default()
            }),
            EventKind::ChatResponse(response) => Some(ChatCompletionMessage {
                role: ChatCompletionMessageRole::Assistant,
                content: Some(response.into_content()),
                ..Default::default()
            }),
            EventKind::ToolCallRequest(request) => Some(ChatCompletionMessage {
                role: ChatCompletionMessageRole::Assistant,
                tool_calls: Some(vec![chat::ToolCall {
                    id: request.id.clone(),
                    r#type: chat::FunctionType::Function,
                    function: ToolCallFunction {
                        name: request.name.clone(),
                        arguments: Value::Object(request.arguments.clone()).to_string(),
                    },
                }]),
                ..Default::default()
            }),
            EventKind::ToolCallResponse(ToolCallResponse { id, result }) => {
                Some(ChatCompletionMessage {
                    role: ChatCompletionMessageRole::Tool,
                    tool_call_id: Some(id),
                    content: Some(match result {
                        Ok(content) | Err(content) => content,
                    }),
                    ..Default::default()
                })
            }
            _ => None,
        })
        .fold(vec![], |mut messages, message| match messages.last_mut() {
            Some(last) if message.tool_calls.is_some() && last.tool_calls.is_some() => {
                last.tool_calls
                    .get_or_insert_default()
                    .extend(message.tool_calls.unwrap_or_default());
                messages
            }
            _ => {
                messages.push(message);
                messages
            }
        })
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
        Vcr::new("http://127.0.0.1:8080", fixtures)
    }

    #[test(tokio::test)]
    async fn test_llamacpp_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().llamacpp;
        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;
                Llamacpp::try_from(&config).unwrap().models().await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_llamacpp_chat_completion() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let mut config = LlmProviderConfig::default().llamacpp;
        let model_id = "llamacpp/llama3:latest".parse().unwrap();
        let model = ModelDetails::empty(model_id);
        let query = ChatQuery {
            thread: Thread {
                events: ConversationStream::default().with_chat_request("Test message"),
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
            |_, url| async move {
                config.base_url = url;

                Llamacpp::try_from(&config)
                    .unwrap()
                    .chat_completion(&model, &ParametersConfig::default(), query)
                    .await
            },
        )
        .await
    }
}
