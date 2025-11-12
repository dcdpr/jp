use std::env;

use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use gemini_client_rs::{GeminiClient, types};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ParametersConfig,
    },
    providers::llm::google::GoogleConfig,
};
use jp_conversation::{
    AssistantMessage, UserMessage,
    event::{ConversationEvent, EventKind},
    thread::{Document, Documents, Thread},
};
use serde_json::Value;
use tracing::trace;

use super::{Event, EventStream, ModelDetails, Provider, ReasoningDetails, Reply};
use crate::{
    CompletionChunk, StreamEvent,
    error::{Error, Result},
    provider::Delta,
    query::ChatQuery,
    stream::{accumulator::Accumulator, event::StreamEndReason},
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Google;

#[derive(Debug, Clone)]
pub struct Google {
    client: GeminiClient,
}

#[async_trait]
impl Provider for Google {
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
        Ok(self
            .client
            .list_models()
            .await?
            .into_iter()
            .map(map_model)
            .collect())
    }

    async fn chat_completion(
        &self,
        model: &ModelDetails,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<Reply> {
        let request = create_request(model, parameters, query)?;

        self.client
            .generate_content(&model.id.name, &request)
            .await
            .map_err(Into::into)
            .and_then(|v| map_response(v, &mut Accumulator::default()))
            .map(|events| Reply {
                provider: PROVIDER,
                events: events
                    .into_iter()
                    .map(|e| match e {
                        StreamEvent::ChatChunk(chunk) => match chunk {
                            CompletionChunk::Content(content) => Event::Content(content),
                            CompletionChunk::Reasoning(reasoning) => Event::Reasoning(reasoning),
                        },
                        StreamEvent::ToolCall(call) => Event::ToolCall(call),
                        StreamEvent::Metadata(key, value) => Event::Metadata(key, value),
                        StreamEvent::EndOfStream(eos) => Event::Finished(eos),
                    })
                    .collect(),
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
        let slug = model.id.name.clone();
        let stream = Box::pin(stream! {
            let mut accumulator = Accumulator::new(200);
            let stream = client
                .stream_content(&slug, &request)
                .await?
                .map_err(Error::from);

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                for event in map_response(event?, &mut accumulator)? {
                    yield Ok(event);
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
) -> Result<types::GenerateContentRequest> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let Thread {
        system_prompt,
        instructions,
        attachments,
        history,
        message,
    } = thread;

    let tools = convert_tools(tools, tool_call_strict_mode);

    #[expect(clippy::cast_possible_wrap)]
    let max_output_tokens = parameters
        .max_tokens
        .or(model.max_output_tokens)
        .map(|v| v as i32);

    let tool_config = (!tools.is_empty()).then_some(convert_tool_choice(tool_choice));
    let reasoning = model.custom_reasoning_config(parameters.reasoning);

    // Add thinking config if the model requires it, or if it supports it,
    // and we have the parameters configured.
    let thinking_config = model
        .reasoning
        .filter(|details| (details.min_tokens() > 0) || reasoning.is_some())
        .map(|details| types::ThinkingConfig {
            include_thoughts: reasoning.is_some_and(|v| !v.exclude),
            thinking_budget: reasoning.map(|v| {
                // TODO: Once the `gemini` crate supports `-1` for "auto"
                // thinking, use that here if `effort` is `Auto`.
                //
                // See: <https://ai.google.dev/gemini-api/docs/thinking#set-budget>
                #[expect(clippy::cast_sign_loss)]
                v.effort
                    .to_tokens(max_output_tokens.unwrap_or(32_000) as u32)
                    .min(details.max_tokens().unwrap_or(u32::MAX))
                    .max(details.min_tokens())
            }),
        });

    let parts = {
        let mut parts = vec![];
        if let Some(text) = system_prompt {
            parts.push(types::ContentData::Text(text));
        }

        if !instructions.is_empty() {
            let text = instructions
                .into_iter()
                .map(|instruction| instruction.try_to_xml().map_err(Into::into))
                .collect::<Result<Vec<_>>>()?
                .join("\n\n");

            parts.push(types::ContentData::Text(text));
        }

        if !attachments.is_empty() {
            let documents: Documents = attachments
                .into_iter()
                .enumerate()
                .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            parts.push(types::ContentData::Text(documents.try_to_xml()?));
        }

        parts
            .into_iter()
            .map(|data| types::ContentPart {
                data,
                thought: false,
                metadata: None,
            })
            .collect::<Vec<_>>()
    };

    Ok(types::GenerateContentRequest {
        system_instruction: if parts.is_empty() {
            None
        } else {
            Some(types::Content { parts, role: None })
        },
        contents: GoogleMessages::build(history, message).0,
        tools,
        tool_config,
        generation_config: Some(types::GenerationConfig {
            max_output_tokens,
            #[expect(clippy::cast_lossless)]
            temperature: parameters.temperature.map(|v| v as f64),
            #[expect(clippy::cast_lossless)]
            top_p: parameters.top_p.map(|v| v as f64),
            #[expect(clippy::cast_possible_wrap)]
            top_k: parameters.top_k.map(|v| v as i32),
            thinking_config,
            ..Default::default()
        }),
    })
}

fn map_model(model: types::Model) -> ModelDetails {
    ModelDetails {
        id: (PROVIDER, model.base_model_id.as_str()).try_into().unwrap(),
        display_name: Some(model.display_name),
        context_window: Some(model.input_token_limit),
        max_output_tokens: Some(model.output_token_limit),
        reasoning: model
            .base_model_id
            .starts_with("gemini-2.5-pro")
            .then_some(ReasoningDetails::supported(128, Some(32768)))
            .or_else(|| {
                model
                    .base_model_id
                    .starts_with("gemini-2.5-flash")
                    .then_some(ReasoningDetails::supported(0, Some(24576)))
            }),
        knowledge_cutoff: None,
        deprecated: None,
        features: vec![],
    }
}

fn map_response(
    response: types::GenerateContentResponse,
    accumulator: &mut Accumulator,
) -> Result<Vec<StreamEvent>> {
    response
        .candidates
        .into_iter()
        .flat_map(|v| map_candidate(v, accumulator))
        .try_fold(vec![], |mut acc, events| {
            acc.extend(events);
            Ok(acc)
        })
}

fn map_candidate(
    candidate: types::Candidate,
    accumulator: &mut Accumulator,
) -> Result<Vec<StreamEvent>> {
    let mut events: Vec<StreamEvent> = candidate
        .content
        .parts
        .into_iter()
        .map(|v| Delta::from(v).into_stream_events(accumulator))
        .try_fold(vec![], |mut acc, events| {
            acc.extend(events?);
            Ok::<_, Error>(acc)
        })?;

    // The model has finished generating tokens, drain the accumulator.
    if candidate.finish_reason.is_some() {
        events.extend(accumulator.drain()?);
    }

    match candidate.finish_reason {
        Some(types::FinishReason::MaxTokens) => {
            events.push(StreamEvent::EndOfStream(StreamEndReason::MaxTokens));
        }
        Some(types::FinishReason::Stop) => {
            events.push(StreamEvent::EndOfStream(StreamEndReason::Completed));
        }
        Some(v) => {
            events.push(StreamEvent::EndOfStream(StreamEndReason::Other(
                serde_json::to_string(&v)?,
            )));
        }
        _ => {}
    }

    Ok(events)
}

impl TryFrom<&GoogleConfig> for Google {
    type Error = Error;

    fn try_from(config: &GoogleConfig) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        Ok(Google {
            client: GeminiClient::new(api_key).with_api_url(config.base_url.clone()),
        })
    }
}

fn convert_tool_choice(choice: ToolChoice) -> types::ToolConfig {
    let (mode, allowed_function_names) = match choice {
        ToolChoice::None => (types::FunctionCallingMode::None, vec![]),
        ToolChoice::Auto => (types::FunctionCallingMode::Auto, vec![]),
        ToolChoice::Required => (types::FunctionCallingMode::Any, vec![]),
        ToolChoice::Function(name) => (types::FunctionCallingMode::Any, vec![name]),
    };

    types::ToolConfig {
        function_calling_config: types::FunctionCallingConfig {
            mode,
            allowed_function_names,
        },
    }
}

fn convert_tools(tools: Vec<ToolDefinition>, _strict: bool) -> Vec<types::Tool> {
    tools
        .into_iter()
        .map(|tool| {
            types::Tool::FunctionDeclaration(types::ToolConfigFunctionDeclaration {
                function_declarations: vec![types::FunctionDeclaration {
                    parameters: None,
                    parameters_json_schema: Some(tool.to_parameters_schema()),
                    name: tool.name,
                    description: tool.description.unwrap_or_default(),
                    response: None,
                }],
            })
        })
        .collect()
}

struct GoogleMessages(Vec<types::Content>);

impl GoogleMessages {
    fn build(history: Vec<ConversationEvent>, message: UserMessage) -> Self {
        // Message history
        let mut items = history
            .into_iter()
            .filter_map(event_to_message)
            .collect::<Vec<_>>();

        // User query
        match message {
            UserMessage::Query { query } => {
                items.push(types::Content {
                    role: Some(types::Role::User),
                    parts: vec![types::ContentData::Text(query).into()],
                });
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| types::Content {
                    role: Some(types::Role::User),
                    parts: vec![types::ContentData::FunctionResponse(
                        types::FunctionResponse {
                            name: result.id,
                            response: types::FunctionResponsePayload {
                                content: serde_json::Value::String(result.content),
                            },
                        },
                    ).into()],
                }));
            }
        }

        Self(items)
    }
}

fn event_to_message(event: ConversationEvent) -> Option<types::Content> {
    match event.kind {
        EventKind::UserMessage(user) => Some(user_message_to_message(user)),
        EventKind::AssistantMessage(assistant) => Some(assistant_message_to_message(assistant)),
        EventKind::ConfigDelta(_) => None,
    }
}

fn user_message_to_message(user: UserMessage) -> types::Content {
    let parts = match user {
        UserMessage::Query { query } => vec![types::ContentData::Text(query).into()],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| {
                types::ContentData::FunctionResponse(types::FunctionResponse {
                    name: result.id,
                    response: types::FunctionResponsePayload {
                        content: Value::String(result.content),
                    },
                })
                .into()
            })
            .collect(),
    };

    types::Content {
        role: Some(types::Role::User),
        parts,
    }
}

fn assistant_message_to_message(assistant: AssistantMessage) -> types::Content {
    let AssistantMessage {
        reasoning,
        content,
        tool_calls,
        ..
    } = assistant;

    let mut parts = vec![];
    if let Some(thinking) = reasoning {
        parts.push(types::ContentPart {
            thought: true,
            data: types::ContentData::Text(thinking),
            metadata: None,
        });
    }

    if let Some(text) = content {
        parts.push(types::ContentData::Text(text).into());
    }

    for tool_call in tool_calls {
        parts.push(
            types::ContentData::FunctionCall(types::FunctionCall {
                name: tool_call.id,
                arguments: Value::Object(tool_call.arguments),
            })
            .into(),
        );
    }

    types::Content {
        role: Some(types::Role::Model),
        parts,
    }
}

impl From<types::ContentPart> for Delta {
    fn from(item: types::ContentPart) -> Self {
        match &item.data {
            types::ContentData::Text(text) if item.thought => Delta::reasoning(text.clone()),
            types::ContentData::Text(text) => Delta::content(text.clone()),
            types::ContentData::InlineData(inline_data) => Delta::content(inline_data.data.clone()),
            types::ContentData::FunctionCall(function_call) => Delta::tool_call(
                function_call.name.clone(),
                function_call.name.clone(),
                function_call.arguments.to_string(),
            )
            .finished(),
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
        Vcr::new("https://generativelanguage.googleapis.com/v1beta", fixtures)
    }

    #[test(tokio::test)]
    async fn test_google_model_details() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().google;
        let name: Name = "gemini-2.5-flash-preview-05-20".parse().unwrap();
        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = format!("{url}/v1beta");
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Google::try_from(&config)
                    .unwrap()
                    .model_details(&name)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_google_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().google;
        let vcr = vcr();
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |recording, url| async move {
                config.base_url = format!("{url}/v1beta");
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Google::try_from(&config).unwrap().models().await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_google_chat_completion() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().google;
        let model_id = "google/gemini-2.5-flash-preview-05-20".parse().unwrap();
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
                config.base_url = format!("{url}/v1beta");
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Google::try_from(&config)
                    .unwrap()
                    .chat_completion(&model, &ParametersConfig::default(), query)
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
    async fn test_google_chat_completion_stream()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().google;
        let model_id = "google/gemini-2.5-flash-preview-05-20".parse().unwrap();
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
                config.base_url = format!("{url}/v1beta");
                if !recording {
                    // dummy api key value when replaying a cassette
                    config.api_key_env = "USER".to_owned();
                }

                Google::try_from(&config)
                    .unwrap()
                    .chat_completion_stream(&model, &ParametersConfig::default(), query)
                    .await
                    .unwrap()
                    .collect::<Vec<_>>()
                    .await
            },
        )
        .await
    }
}
