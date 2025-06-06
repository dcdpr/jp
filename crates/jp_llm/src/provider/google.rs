use std::env;

use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use gemini_client_rs::{types, GeminiClient};
use jp_config::llm;
use jp_conversation::{
    model::ProviderId,
    thread::{Document, Documents, Thread},
    AssistantMessage, MessagePair, Model, UserMessage,
};
use jp_mcp::tool;
use jp_query::query::ChatQuery;
use serde_json::Value;
use tracing::trace;

use super::{Event, EventStream, ModelDetails, Provider, ReasoningDetails};
use crate::{
    error::{Error, Result},
    provider::Delta,
};

#[derive(Debug, Clone)]
pub struct Google {
    client: GeminiClient,
}

impl Google {
    async fn create_request(
        &self,
        model: &Model,
        query: ChatQuery,
    ) -> Result<types::GenerateContentRequest> {
        let ChatQuery {
            thread,
            tools,
            tool_choice,
            tool_call_strict_mode,
        } = query;

        let details = self
            .models()
            .await?
            .into_iter()
            .find(|m| m.slug == model.id.slug());

        let system_prompt = thread.system_prompt.clone();
        let tools = convert_tools(tools, tool_call_strict_mode);

        #[expect(clippy::cast_possible_wrap)]
        let max_output_tokens = model
            .parameters
            .max_tokens
            .or_else(|| details.as_ref().and_then(|d| d.max_output_tokens))
            .map(|v| v as i32);

        let tool_config = (!tools.is_empty()).then_some(convert_tool_choice(tool_choice));

        // Add thinking config if the model requires it, or if it supports it,
        // and we have the parameters configured.
        let thinking_config = details
            .as_ref()
            .and_then(|d| d.reasoning)
            .filter(|details| (details.min_tokens > 0) || model.parameters.reasoning.is_some())
            .map(|details| types::ThinkingConfig {
                include_thoughts: model.parameters.reasoning.is_some_and(|v| !v.exclude),
                thinking_budget: model.parameters.reasoning.map(|v| {
                    #[expect(clippy::cast_sign_loss)]
                    v.effort
                        .to_tokens(max_output_tokens.unwrap_or(32_000) as u32)
                        .min(details.max_tokens.unwrap_or(u32::MAX))
                        .max(details.min_tokens)
                }),
            });

        Ok(types::GenerateContentRequest {
            system_instruction: system_prompt.map(|text| types::Content {
                parts: vec![types::ContentData::Text(text).into()],
                role: types::Role::System,
            }),
            contents: convert_thread(thread)?,
            tools,
            tool_config,
            generation_config: Some(types::GenerationConfig {
                max_output_tokens,
                #[expect(clippy::cast_lossless)]
                temperature: model.parameters.temperature.map(|v| v as f64),
                #[expect(clippy::cast_lossless)]
                top_p: model.parameters.top_p.map(|v| v as f64),
                #[expect(clippy::cast_possible_wrap)]
                top_k: model.parameters.top_k.map(|v| v as i32),
                thinking_config,
                ..Default::default()
            }),
        })
    }
}

#[async_trait]
impl Provider for Google {
    async fn models(&self) -> Result<Vec<ModelDetails>> {
        Ok(self
            .client
            .list_models()
            .await?
            .into_iter()
            .map(map_model)
            .collect())
    }

    async fn chat_completion(&self, model: &Model, query: ChatQuery) -> Result<Vec<Event>> {
        let request = self.create_request(model, query).await?;

        self.client
            .generate_content(model.id.slug(), &request)
            .await
            .map_err(Into::into)
            .and_then(map_response)
    }

    async fn chat_completion_stream(&self, model: &Model, query: ChatQuery) -> Result<EventStream> {
        let client = self.client.clone();
        let request = self.create_request(model, query).await?;
        let slug = model.id.slug().to_owned();
        let stream = Box::pin(stream! {
            let stream = client
                .stream_content(&slug, &request)
                .await?
                .map_err(Error::from);

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                for event in map_response(event?)? {
                    yield Ok(event.into());
                }
            }
        });

        Ok(stream)
    }
}

#[expect(clippy::needless_pass_by_value)]
fn map_model(model: types::Model) -> ModelDetails {
    ModelDetails {
        provider: ProviderId::Google,
        slug: model.base_model_id.clone(),
        context_window: Some(model.input_token_limit),
        max_output_tokens: Some(model.output_token_limit),
        reasoning: model
            .base_model_id
            .starts_with("gemini-2.5-pro")
            .then_some(ReasoningDetails {
                supported: true,
                min_tokens: 128,
                max_tokens: Some(32768),
            })
            .or_else(|| {
                model
                    .base_model_id
                    .starts_with("gemini-2.5-flash")
                    .then_some(ReasoningDetails {
                        supported: true,
                        min_tokens: 0,
                        max_tokens: Some(32768),
                    })
            }),
        knowledge_cutoff: None,
    }
}

fn map_response(response: types::GenerateContentResponse) -> Result<Vec<Event>> {
    response
        .candidates
        .into_iter()
        .flat_map(|v| v.content.parts)
        .filter_map(|v| Option::<Result<Event>>::from(Delta::from(v)))
        .collect::<Result<_>>()
}

impl TryFrom<&llm::provider::google::Config> for Google {
    type Error = Error;

    fn try_from(config: &llm::provider::google::Config) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        Ok(Google {
            client: GeminiClient::new(api_key).with_api_url(config.base_url.clone()),
        })
    }
}

fn convert_tool_choice(choice: tool::ToolChoice) -> types::ToolConfig {
    let (mode, allowed_function_names) = match choice {
        tool::ToolChoice::None => (types::FunctionCallingMode::None, vec![]),
        tool::ToolChoice::Auto => (types::FunctionCallingMode::Auto, vec![]),
        tool::ToolChoice::Required => (types::FunctionCallingMode::Any, vec![]),
        tool::ToolChoice::Function(name) => (types::FunctionCallingMode::Any, vec![name]),
    };

    types::ToolConfig {
        function_calling_config: types::FunctionCallingConfig {
            mode,
            allowed_function_names,
        },
    }
}

fn convert_tools(tools: Vec<jp_mcp::Tool>, _strict: bool) -> Vec<types::Tool> {
    tools
        .into_iter()
        .map(|tool| {
            types::Tool::FunctionDeclaration(types::ToolConfigFunctionDeclaration {
                function_declarations: vec![types::FunctionDeclaration {
                    name: tool.name.to_string(),
                    description: tool.description.unwrap_or_default().to_string(),
                    parameters: Some(Value::Object(tool.input_schema.as_ref().clone())),
                    response: None,
                }],
            })
        })
        .collect()
}

fn convert_thread(thread: Thread) -> Result<Vec<types::Content>> {
    Messages::try_from(thread).map(|v| v.0)
}

struct Messages(Vec<types::Content>);

impl TryFrom<Thread> for Messages {
    type Error = Error;

    fn try_from(thread: Thread) -> Result<Self> {
        let Thread {
            instructions,
            attachments,
            mut history,
            message,
            ..
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
            items.push(types::Content {
                role: types::Role::User,
                parts: content
                    .into_iter()
                    .map(|s| types::ContentData::Text(s).into())
                    .collect(),
            });
        }

        if items
            .last()
            .is_some_and(|m| matches!(m.role, types::Role::User))
        {
            items.push(types::Content {
                role: types::Role::Model,
                parts: vec![types::ContentData::Text(
                    "Thank you for those details, I'll use them to inform my next response.".into(),
                )
                .into()],
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
                items.push(types::Content {
                    role: types::Role::User,
                    parts: vec![types::ContentData::Text(text).into()],
                });
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| types::Content {
                    role: types::Role::User,
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

        Ok(Self(items))
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<types::Content> {
    let (user, assistant) = msg.split();

    vec![
        user_message_to_message(user),
        assistant_message_to_message(assistant),
    ]
}

fn user_message_to_message(user: UserMessage) -> types::Content {
    let parts = match user {
        UserMessage::Query(query) => vec![types::ContentData::Text(query).into()],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| {
                types::ContentData::FunctionResponse(types::FunctionResponse {
                    name: result.id,
                    response: types::FunctionResponsePayload {
                        content: serde_json::Value::String(result.content),
                    },
                })
                .into()
            })
            .collect(),
    };

    types::Content {
        role: types::Role::User,
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
                arguments: tool_call.arguments,
            })
            .into(),
        );
    }

    types::Content {
        role: types::Role::Model,
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

    use jp_conversation::ModelId;
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr() -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new("https://generativelanguage.googleapis.com/v1beta", fixtures)
    }

    #[test(tokio::test)]
    async fn test_google_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.google;
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
        let mut config = llm::Config::default().provider.google;
        let model: ModelId = "google/gemini-2.5-flash-preview-05-20".parse().unwrap();
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
                    .chat_completion(&model.into(), query)
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
    async fn test_google_chat_completion_stream(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.google;
        let model: ModelId = "google/gemini-2.5-flash-preview-05-20".parse().unwrap();
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
                config.base_url = format!("{url}/v1beta");

                Google::try_from(&config)
                    .unwrap()
                    .chat_completion_stream(&model.into(), query)
                    .await
                    .unwrap()
                    .filter_map(
                        |r| async move { r.unwrap().into_chat_chunk().unwrap().into_content() },
                    )
                    .collect::<String>()
                    .await
            },
        )
        .await
    }
}
