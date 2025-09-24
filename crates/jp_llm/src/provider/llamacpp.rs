use std::mem;

use async_stream::stream;
use async_trait::async_trait;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, ProviderId},
        parameters::ParametersConfig,
    },
    providers::llm::llamacpp::LlamacppConfig,
};
use jp_conversation::{
    thread::{Document, Documents, Thread},
    AssistantMessage, MessagePair, UserMessage,
};
use openai::{
    chat::{
        self, structured_output::ToolCallFunctionDefinition, ChatCompletionBuilder,
        ChatCompletionChoiceDelta, ChatCompletionDelta, ChatCompletionGeneric,
        ChatCompletionMessage, ChatCompletionMessageDelta, ChatCompletionMessageRole,
        ToolCallFunction,
    },
    Credentials,
};
use serde::Serialize;
use tracing::{debug, trace};

use super::{
    openai::{ModelListResponse, ModelResponse},
    CompletionChunk, Delta, EventStream, ModelDetails, StreamEvent,
};
use crate::{
    error::{Error, Result},
    provider::{handle_delta, AccumulationState, Provider, ReasoningExtractor},
    query::ChatQuery,
    tool::ToolDefinition,
};

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
        model_id: &ModelIdConfig,
        _parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<ChatCompletionBuilder> {
        let slug = model_id.name.to_string();
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
            .iter()
            .map(map_model)
            .collect())
    }

    async fn chat_completion_stream(
        &self,
        model_id: &ModelIdConfig,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<EventStream> {
        debug!(
            model = %model_id.name,
            "Starting Llamacpp chat completion stream."
        );

        let request = self.build_request(model_id, parameters, query)?;
        let stream = Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            let mut extractor = ReasoningExtractor::default();

            let stream = request
                .create_stream()
                .await.expect("Should not fail to clone");
            tokio::pin!(stream);

            while let Some(delta) = stream.recv().await {
                let delta = delta.choices.into_iter().next().map(|c| (c.delta, c.finish_reason));
                let Some((delta, finish_reason)) = delta else {
                    continue
                };

                extractor.handle(delta.content.as_deref().unwrap_or_default());

                let tool_call_finished = finish_reason.is_some_and(|reason| reason == "function_call");
                for event in map_event(delta, &mut current_state, &mut extractor, tool_call_finished) {
                    yield event;
                }
            }

            extractor.finalize();

            if current_state.is_accumulating() && let Some(event) =
                handle_delta(Delta::tool_call_finished(), &mut current_state).transpose() {
                    yield event;
            }

            for event in map_content(&mut current_state, &mut extractor) {
                yield event;
            }
        });

        Ok(stream)
    }
}

fn map_event(
    event: ChatCompletionMessageDelta,
    state: &mut AccumulationState,
    extractor: &mut ReasoningExtractor,
    tool_call_finished: bool,
) -> Vec<Result<StreamEvent>> {
    let mut events = vec![];

    for tool_call in event.tool_calls.into_iter().flatten() {
        let mut delta = Delta::tool_call(
            tool_call.id.clone().unwrap_or_default(),
            tool_call
                .function
                .as_ref()
                .map(|f| f.name.clone())
                .unwrap_or_default(),
            tool_call
                .function
                .as_ref()
                .map(|f| f.arguments.clone())
                .unwrap_or_default(),
        );

        if tool_call_finished {
            delta.tool_call_finished = true;
        }

        events.extend(handle_delta(delta, state).transpose());
    }

    events.extend(map_content(state, extractor));
    events
}

fn map_content(
    state: &mut AccumulationState,
    extractor: &mut ReasoningExtractor,
) -> Vec<Result<StreamEvent>> {
    let mut events = Vec::new();
    if !extractor.reasoning.is_empty() {
        let reasoning = mem::take(&mut extractor.reasoning);
        events.extend(handle_delta(Delta::reasoning(reasoning), state).transpose());
    }

    if !extractor.other.is_empty() {
        let content = mem::take(&mut extractor.other);
        events.extend(handle_delta(Delta::content(content), state).transpose());
    }

    events
}

fn map_model(model: &ModelResponse) -> ModelDetails {
    ModelDetails {
        provider: ProviderId::Llamacpp,
        slug: model
            .id
            .rsplit_once('/')
            .map_or(model.id.as_str(), |(_, v)| v)
            .to_string(),
        context_window: None,
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: None,
    }
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
    Messages::try_from(thread).map(|v| v.0)
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Messages(pub Vec<ChatCompletionMessage>);

impl TryFrom<Thread> for Messages {
    type Error = Error;

    fn try_from(thread: Thread) -> Result<Self> {
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
            .flat_map(message_pair_to_messages)
            .collect::<Vec<_>>();

        // System message first, if any.
        if let Some(system_prompt) = system_prompt {
            items.push(ChatCompletionMessage {
                role: ChatCompletionMessageRole::System,
                content: Some(system_prompt),
                ..Default::default()
            });
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
            items.push(ChatCompletionMessage {
                role: ChatCompletionMessageRole::User,
                content: Some(content.join("\n\n")),
                ..Default::default()
            });
        }

        if items
            .last()
            .is_some_and(|m| matches!(m.role, ChatCompletionMessageRole::User))
        {
            items.push(ChatCompletionMessage {
                role: ChatCompletionMessageRole::Assistant,
                content: Some(
                    "Thank you for those details, I'll use them to inform my next response.".into(),
                ),
                ..Default::default()
            });
        }

        items.extend(
            history_after_instructions
                .into_iter()
                .flat_map(message_pair_to_messages),
        );

        // User query
        match message {
            UserMessage::Query(text) => {
                items.push(ChatCompletionMessage {
                    role: ChatCompletionMessageRole::User,
                    content: Some(text),
                    ..Default::default()
                });
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| ChatCompletionMessage {
                    role: ChatCompletionMessageRole::Tool,
                    content: Some(result.content),
                    ..Default::default()
                }));
            }
        }

        Ok(Self(items))
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<ChatCompletionMessage> {
    let (user, assistant) = msg.split();

    user_message_to_messages(user)
        .into_iter()
        .chain(Some(assistant_message_to_message(assistant)))
        .collect()
}

fn user_message_to_messages(user: UserMessage) -> Vec<ChatCompletionMessage> {
    match user {
        UserMessage::Query(query) if !query.is_empty() => vec![ChatCompletionMessage {
            role: ChatCompletionMessageRole::User,
            content: Some(query),
            ..Default::default()
        }],
        UserMessage::Query(_) => vec![],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| ChatCompletionMessage {
                role: ChatCompletionMessageRole::Tool,
                content: Some(result.content),
                tool_call_id: Some(result.id),
                ..Default::default()
            })
            .collect(),
    }
}

fn assistant_message_to_message(assistant: AssistantMessage) -> ChatCompletionMessage {
    let AssistantMessage {
        content,
        tool_calls,
        ..
    } = assistant;

    let mut message = ChatCompletionMessage {
        role: ChatCompletionMessageRole::Assistant,
        content,
        tool_calls: Some(
            tool_calls
                .into_iter()
                .map(|call| chat::ToolCall {
                    id: call.id,
                    r#type: chat::FunctionType::Function,
                    function: ToolCallFunction {
                        name: call.name,
                        arguments: call.arguments.to_string(),
                    },
                })
                .collect(),
        ),
        ..Default::default()
    };

    if message.content.as_ref().is_none_or(String::is_empty)
        && message.tool_calls.as_ref().is_none_or(Vec::is_empty)
    {
        message.content = Some("<no response>".to_owned());
    }

    message
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
            |_, url| async move {
                config.base_url = url;

                Llamacpp::try_from(&config)
                    .unwrap()
                    .chat_completion(&model_id, &ParametersConfig::default(), query)
                    .await
            },
        )
        .await
    }
}
