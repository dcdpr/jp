use std::{mem, str::FromStr as _};

use async_trait::async_trait;
use futures::{FutureExt as _, StreamExt as _, future, stream};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ReasoningConfig,
    },
    providers::llm::ollama::OllamaConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatResponse, EventKind, ToolCallRequest},
};
use ollama_rs::{
    Ollama as Client,
    generation::{
        chat::{ChatMessage, ChatMessageResponse, MessageRole, request::ChatMessageRequest},
        parameters::{KeepAlive, TimeUnit},
        tools::{ToolCall, ToolCallFunction, ToolFunctionInfo, ToolInfo, ToolType},
    },
    models::{LocalModel, ModelOptions},
};
use serde_json::{Map, Value};
use tracing::{debug, trace};
use url::Url;

use super::{EventStream, ModelDetails, Provider};
use crate::{
    error::{Error, Result},
    event::{Event, FinishReason},
    query::ChatQuery,
    stream::aggregator::reasoning::ReasoningExtractor,
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Ollama;

#[derive(Debug, Clone)]
pub struct Ollama {
    client: Client,
}

#[async_trait]
impl Provider for Ollama {
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
        let models = self.client.list_local_models().await?;

        models.into_iter().map(map_model).collect::<Result<_>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream> {
        debug!(
            model = %model.id.name,
            "Starting Ollama chat completion stream."
        );

        let mut extractor = ReasoningExtractor::default();
        let request = create_request(model, query)?;

        trace!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            "Sending request to Ollama."
        );

        Ok(self
            .client
            .send_chat_messages_stream(request)
            .await?
            .filter_map(|v| future::ready(v.ok()))
            .map(move |v| stream::iter(map_event(v, &mut extractor)))
            .flatten()
            .chain(future::ready(Event::Finished(FinishReason::Completed)).into_stream())
            .map(Ok)
            .boxed())
    }
}

fn map_model(model: LocalModel) -> Result<ModelDetails> {
    Ok(ModelDetails {
        id: (PROVIDER, &model.name).try_into()?,
        display_name: Some(model.name),
        context_window: None,
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        features: vec![],
    })
}

fn map_event(event: ChatMessageResponse, extractor: &mut ReasoningExtractor) -> Vec<Event> {
    let ChatMessageResponse { message, done, .. } = event;

    let mut events = fetch_content(extractor, done);

    for (
        index,
        ToolCall {
            function: ToolCallFunction { name, arguments },
        },
    ) in message.tool_calls.into_iter().enumerate()
    {
        events.extend(vec![
            Event::Part {
                // These events don't have any index assigned, but we use `0`
                // and `1` for regular chat messages and reasoning, and `2` and
                // up for tool calls.
                index: index + 2,
                event: ConversationEvent::now(ToolCallRequest {
                    id: String::new(),
                    name,
                    arguments: match arguments {
                        Value::Object(map) => map,
                        v => Map::from_iter([("input".into(), v)]),
                    },
                }),
            },
            Event::flush(0),
        ]);
    }

    events
}

fn fetch_content(extractor: &mut ReasoningExtractor, done: bool) -> Vec<Event> {
    let mut events = Vec::new();

    if !extractor.reasoning.is_empty() {
        let reasoning = mem::take(&mut extractor.reasoning);
        events.push(Event::Part {
            index: 0,
            event: ConversationEvent::now(ChatResponse::reasoning(reasoning)),
        });
    }

    if !extractor.other.is_empty() {
        let content = mem::take(&mut extractor.other);
        events.push(Event::Part {
            index: 1,
            event: ConversationEvent::now(ChatResponse::message(content)),
        });
    }

    if done {
        events.extend(vec![Event::flush(0), Event::flush(1)]);
    }

    events
}

fn create_request(model: &ModelDetails, query: ChatQuery) -> Result<ChatMessageRequest> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let config = thread.events.config()?;
    let parameters = &config.assistant.model.parameters;

    let mut messages = thread.into_messages(to_system_messages, convert_events)?;

    if let Some(tool_choice) = tool_choice_to_system_message(&tool_choice) {
        messages.push(tool_choice);
    }

    let mut request = ChatMessageRequest::new(model.id.name.to_string(), messages);

    let tools = convert_tools(tools, tool_call_strict_mode)?;
    if !tools.is_empty() {
        request = request.tools(tools);
    }

    let mut options = ModelOptions::default();

    if let Some(temperature) = parameters.temperature {
        options = options.temperature(temperature);
    }

    if let Some(top_p) = parameters.top_p {
        options = options.top_p(top_p);
    }

    if let Some(top_k) = parameters.top_k {
        options = options.top_k(top_k);
    }

    // Set the context window for the model.
    //
    // This can be used to force Ollama to use a larger context window then the
    // one determined based on the machine's resources.
    if let Some(context_window) = parameters
        .other
        .get("context_window")
        .and_then(Value::as_u64)
    {
        options = options.num_ctx(context_window);
    }

    if let Some(keep_alive) = parameters.other.get("keep_alive").and_then(Value::as_str) {
        let unit = keep_alive
            .chars()
            .last()
            .filter(char::is_ascii_alphabetic)
            .unwrap_or('m');

        let value = keep_alive
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>();

        request = request.keep_alive(KeepAlive::Until {
            time: value.parse::<u64>().unwrap_or(5),
            unit: match unit {
                's' => TimeUnit::Seconds,
                'h' => TimeUnit::Hours,
                _ => TimeUnit::Minutes,
            },
        });
    }

    request = request.options(options);

    // Reasoning for local models has to be explicitly enabled. This is because
    // there are too many models that do not support reasoning, and we have no
    // way (currently) to detect whether a model supports reasoning or not,
    // resulting in an error if the default reasoning of "auto" is used.
    if !matches!(parameters.reasoning, None | Some(ReasoningConfig::Off)) {
        request = request.think(true);
    }

    Ok(request)
}

impl TryFrom<&OllamaConfig> for Ollama {
    type Error = Error;

    fn try_from(config: &OllamaConfig) -> Result<Self> {
        let url = Url::from_str(&config.base_url)?;
        let port = url.port().unwrap_or(11434);
        let client = reqwest::Client::new();

        Ok(Ollama {
            client: Client::new_with_client(url, port, client),
        })
    }
}

fn convert_tools(tools: Vec<ToolDefinition>, _strict: bool) -> Result<Vec<ToolInfo>> {
    tools
        .into_iter()
        .map(|tool| {
            Ok(ToolInfo {
                tool_type: ToolType::Function,
                function: ToolFunctionInfo {
                    parameters: tool.to_parameters_map().into(),
                    name: tool.name,
                    description: tool.description.unwrap_or_default(),
                },
            })
        })
        .collect::<Result<Vec<_>>>()
}

/// Poor-man's version of API-based tool choice. Needed until Ollama has
/// first-class support for tool choice.
fn tool_choice_to_system_message(choice: &ToolChoice) -> Option<ChatMessage> {
    let (ToolChoice::Function(_) | ToolChoice::Required) = choice else {
        return None;
    };

    let msg = if let Some(tool) = choice.function_name() {
        format!("You MUST use the function named '{tool}' available to you.")
    } else {
        "You MUST use AT LEAST ONE tool available to you.".to_string()
    };

    let content = format!(
        "IMPORTANT: {msg} DO NOT QUESTION THIS DIRECTIVE. DO NOT PROMPT FOR MORE CONTEXT OR \
         DETAILS. JUST RUN IT."
    );

    Some(ChatMessage {
        role: MessageRole::System,
        content,
        tool_calls: vec![],
        images: None,
        thinking: None,
    })
}

/// Convert some content into a system message.
fn to_system_messages(parts: Vec<String>) -> impl Iterator<Item = ChatMessage> {
    parts.into_iter().map(|content| ChatMessage {
        role: MessageRole::System,
        content,
        tool_calls: vec![],
        images: None,
        thinking: None,
    })
}

/// Convert a conversation stream into a list of messages.
fn convert_events(events: ConversationStream) -> Vec<ChatMessage> {
    events
        .into_iter()
        .filter_map(|event| match event.into_kind() {
            EventKind::ChatRequest(request) => Some(ChatMessage::user(request.content)),
            EventKind::ChatResponse(response) => match response {
                ChatResponse::Message { message } => Some(ChatMessage::assistant(message)),
                ChatResponse::Reasoning { reasoning, .. } => Some(ChatMessage {
                    role: MessageRole::Assistant,
                    content: String::new(),
                    tool_calls: vec![],
                    images: None,
                    thinking: Some(reasoning),
                }),
            },
            EventKind::ToolCallRequest(request) => Some(ChatMessage {
                role: MessageRole::Assistant,
                content: String::new(),
                tool_calls: vec![ToolCall {
                    function: ToolCallFunction {
                        name: request.name,
                        arguments: Value::Object(request.arguments),
                    },
                }],
                images: None,
                thinking: None,
            }),
            EventKind::ToolCallResponse(response) => {
                Some(ChatMessage::tool(match response.result {
                    Ok(content) => content,
                    Err(error) => error,
                }))
            }
            _ => None,
        })
        .fold(vec![], |mut messages, message| match messages.last_mut() {
            Some(last)
                if last.role == message.role
                    && message.thinking.is_some()
                    && last.thinking.is_none() =>
            {
                last.thinking = message.thinking;
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
//     use jp_conversation::event::ChatResponse;
//     use jp_test::{Result, fn_name, mock::Vcr};
//     use test_log::test;
//
//     use super::*;
//     use crate::structured;
//
//     fn vcr(url: &str) -> Vcr {
//         Vcr::new(url, env!("CARGO_MANIFEST_DIR"))
//     }
//
//     #[test(tokio::test)]
//     async fn test_ollama_models() -> Result {
//         let mut config = LlmProviderConfig::default().ollama;
//         let vcr = vcr(&config.base_url);
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
//                 Ollama::try_from(&config)
//                     .unwrap()
//                     .models()
//                     .await
//                     .map(|mut v| {
//                         v.truncate(2);
//                         v
//                     })
//             },
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_ollama_chat_completion() -> Result {
//         let mut config = LlmProviderConfig::default().ollama;
//         let model_id = "ollama/llama3:latest".parse().unwrap();
//         let model = ModelDetails::empty(model_id);
//         let query = ChatQuery {
//             thread: Thread {
//                 events: ConversationStream::default().with_chat_request("Test message"),
//                 ..Default::default()
//             },
//             ..Default::default()
//         };
//
//         let vcr = vcr(&config.base_url);
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
//                 Ollama::try_from(&config)
//                     .unwrap()
//                     .chat_completion(&model, query)
//                     .await
//             },
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_ollama_chat_completion_stream() -> Result {
//         let mut config = LlmProviderConfig::default().ollama;
//         let model_id = "ollama/llama3:latest".parse().unwrap();
//         let model = ModelDetails::empty(model_id);
//         let query = ChatQuery {
//             thread: Thread {
//                 events: ConversationStream::default().with_chat_request("Test message"),
//                 ..Default::default()
//             },
//             ..Default::default()
//         };
//
//         let vcr = vcr(&config.base_url);
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
//                 Ollama::try_from(&config)
//                     .unwrap()
//                     .chat_completion_stream(&model, query)
//                     .await
//                     .unwrap()
//                     .collect::<Vec<_>>()
//                     .await
//             },
//         )
//         .await
//     }
//
//     #[test(tokio::test)]
//     async fn test_ollama_structured_completion() -> Result {
//         let mut config = LlmProviderConfig::default().ollama;
//         let model_id = "ollama/llama3.1:8b".parse().unwrap();
//         let model = ModelDetails::empty(model_id);
//         let history = ConversationStream::default()
//             .with_chat_request("Test message")
//             .with_chat_response(ChatResponse::reasoning("Test response"));
//
//         let vcr = vcr(&config.base_url);
//         vcr.cassette(
//             fn_name!(),
//             |rule| {
//                 rule.filter(|when| {
//                     when.any_request();
//                 });
//             },
//             |_, url| async move {
//                 config.base_url = url;
//                 let query = structured::titles::titles(3, history, &[]).unwrap();
//
//                 Ollama::try_from(&config)
//                     .unwrap()
//                     .structured_completion(&model, query)
//                     .await
//             },
//         )
//         .await
//     }
//
//     mod chunk_parser {
//         use test_log::test;
//
//         use super::*;
//
//         #[test]
//         fn test_no_think_tag_at_all() {
//             let mut parser = ReasoningExtractor::default();
//             parser.handle("some other text");
//             parser.finalize();
//             assert_eq!(parser.other, "some other text");
//             assert_eq!(parser.reasoning, "");
//         }
//
//         #[test]
//         fn test_standard_case_with_newline() {
//             let mut parser = ReasoningExtractor::default();
//             parser.handle("prefix\n<think>\nthoughts\n</think>\nsuffix");
//             parser.finalize();
//             assert_eq!(parser.reasoning, "thoughts\n");
//             assert_eq!(parser.other, "prefix\nsuffix");
//         }
//
//         #[test]
//         fn test_suffix_only() {
//             let mut parser = ReasoningExtractor::default();
//             parser.handle("<think>\nthoughts\n</think>\n\nsuffix text here");
//             parser.finalize();
//             assert_eq!(parser.reasoning, "thoughts\n");
//             assert_eq!(parser.other, "\nsuffix text here");
//         }
//
//         #[test]
//         fn test_ends_with_closing_tag_no_newline() {
//             let mut parser = ReasoningExtractor::default();
//             parser.handle("<think>\nfinal thoughts\n");
//             parser.handle("</think>");
//             parser.finalize();
//             assert_eq!(parser.reasoning, "final thoughts\n");
//             assert_eq!(parser.other, "");
//         }
//
//         #[test]
//         fn test_less_than_symbol_in_reasoning_content_is_not_stripped() {
//             let mut parser = ReasoningExtractor::default();
//             parser.handle("<think>\na < b is a true statement\n</think>");
//             parser.finalize();
//             // The last '<' is part of "</think>", so "a < b is a true statement" is kept.
//             assert_eq!(parser.reasoning, "a < b is a true statement\n");
//         }
//
//         #[test]
//         fn test_less_than_symbol_not_part_of_tag_is_kept() {
//             let mut parser = ReasoningExtractor::default();
//             parser.handle("<think>\nhere is a random < symbol");
//             parser.finalize();
//             // The final '<' is not a prefix of '</think>', so it's kept.
//             assert_eq!(parser.reasoning, "here is a random < symbol");
//         }
//     }
// }
