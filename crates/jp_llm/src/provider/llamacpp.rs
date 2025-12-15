use std::mem;

use async_trait::async_trait;
use futures::{FutureExt as _, StreamExt as _, future, stream};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::id::{ModelIdConfig, Name, ProviderId},
    providers::llm::llamacpp::LlamacppConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatResponse, EventKind, ToolCallResponse},
};
use openai::{
    Credentials,
    chat::{
        self, ChatCompletionBuilder, ChatCompletionChoiceDelta, ChatCompletionDelta,
        ChatCompletionMessage, ChatCompletionMessageDelta, ChatCompletionMessageRole,
        ToolCallFunction, structured_output::ToolCallFunctionDefinition,
    },
};
use serde_json::Value;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, trace};

use super::{
    EventStream, ModelDetails,
    openai::{ModelListResponse, ModelResponse},
};
use crate::{
    error::{Error, Result},
    event::{Event, FinishReason},
    provider::{Provider, openai::parameters_with_strict_mode},
    query::ChatQuery,
    stream::aggregator::{
        reasoning::ReasoningExtractor, tool_call_request::ToolCallRequestAggregator,
    },
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
        query: ChatQuery,
    ) -> Result<ChatCompletionBuilder> {
        let slug = model.id.name.to_string();
        let ChatQuery {
            thread,
            tools,
            tool_choice,
            tool_call_strict_mode,
        } = query;

        let messages = thread.into_messages(to_system_messages, convert_events)?;
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
        query: ChatQuery,
    ) -> Result<EventStream> {
        debug!(
            model = %model.id.name,
            "Starting Llamacpp chat completion stream."
        );

        let mut extractor = ReasoningExtractor::default();
        let mut agg = ToolCallRequestAggregator::default();
        let request = self.build_request(model, query)?;

        trace!(?request, "Sending request to Llamacpp.");

        Ok(request
            .create_stream()
            .await
            .map(ReceiverStream::new)
            .expect("Should not fail to clone")
            .flat_map(|v| stream::iter(v.choices))
            .flat_map(move |v| stream::iter(map_event(v, &mut extractor, &mut agg)))
            .chain(future::ok(Event::Finished(FinishReason::Completed)).into_stream())
            .boxed())
    }
}

fn map_event(
    event: ChatCompletionChoiceDelta,
    extractor: &mut ReasoningExtractor,
    agg: &mut ToolCallRequestAggregator,
) -> Vec<Result<Event>> {
    let ChatCompletionChoiceDelta {
        index,
        finish_reason,
        delta:
            ChatCompletionMessageDelta {
                content,
                tool_calls,
                ..
            },
    } = event;

    #[allow(clippy::cast_possible_truncation)]
    let index = index as usize;
    let mut events = vec![];

    for chat::ToolCallDelta { id, function, .. } in tool_calls.into_iter().flatten() {
        let (name, arguments) = match function {
            Some(chat::ToolCallFunction { name, arguments }) => (Some(name), Some(arguments)),
            None => (None, None),
        };

        agg.add_chunk(index, id, name, arguments.as_deref());
    }

    if ["function_call", "tool_calls"].contains(&finish_reason.as_deref().unwrap_or_default()) {
        match agg.finalize(index) {
            Ok(request) => events.extend(vec![
                Ok(Event::Part {
                    index,
                    event: ConversationEvent::now(request),
                }),
                Ok(Event::flush(index)),
            ]),
            Err(error) => events.push(Err(error.into())),
        }
    }

    if let Some(content) = content {
        extractor.handle(&content);
    }

    if finish_reason.is_some() {
        extractor.finalize();
    }

    events.extend(fetch_content(extractor, index).into_iter().map(Ok));
    events
}

fn fetch_content(extractor: &mut ReasoningExtractor, index: usize) -> Vec<Event> {
    let mut events = Vec::new();
    if !extractor.reasoning.is_empty() {
        let reasoning = mem::take(&mut extractor.reasoning);
        events.push(Event::Part {
            index,
            event: ConversationEvent::now(ChatResponse::reasoning(reasoning)),
        });
    }

    if !extractor.other.is_empty() {
        let content = mem::take(&mut extractor.other);
        events.push(Event::Part {
            index,
            event: ConversationEvent::now(ChatResponse::message(content)),
        });
    }

    events
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
                parameters: Some(Value::Object(parameters_with_strict_mode(
                    tool.parameters,
                    strict,
                ))),
                name: tool.name,
                description: tool.description.or(Some(String::new())),
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

/// Convert a list of content into system messages.
fn to_system_messages(parts: Vec<String>) -> impl Iterator<Item = ChatCompletionMessage> {
    parts.into_iter().map(|content| ChatCompletionMessage {
        role: ChatCompletionMessageRole::System,
        content: Some(content),
        ..Default::default()
    })
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

// #[cfg(test)]
// mod tests {
//     use jp_config::providers::llm::LlmProviderConfig;
//     use jp_test::{Result, fn_name, mock::Vcr};
//     use test_log::test;
//
//     use super::*;
//
//     fn vcr() -> Vcr {
//         Vcr::new("http://127.0.0.1:8080", env!("CARGO_MANIFEST_DIR"))
//     }
//
//     #[test(tokio::test)]
//     async fn test_llamacpp_models() -> Result {
//         let mut config = LlmProviderConfig::default().llamacpp;
//         let vcr = vcr();
//         vcr.cassette(
//             fn_name!(),
//             |rule| {
//                 rule.filter(|when| {
//                     when.any_request();
//                 });
//             },
//             |_, url| async move {
//                 config.base_url = url;
//                 Llamacpp::try_from(&config).unwrap().models().await
//             },
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_llamacpp_chat_completion() -> Result {
//         let mut config = LlmProviderConfig::default().llamacpp;
//         let model_id = "llamacpp/llama3:latest".parse().unwrap();
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
//             |_, url| async move {
//                 config.base_url = url;
//
//                 Llamacpp::try_from(&config)
//                     .unwrap()
//                     .chat_completion(&model, query)
//                     .await
//             },
//         )
//         .await
//     }
// }
