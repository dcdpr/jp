use std::{env, time::Duration};

use async_anthropic::{
    errors::AnthropicError,
    messages::DEFAULT_MAX_TOKENS,
    types::{
        self, ListModelsResponse, System, Thinking, ToolBash, ToolCodeExecution, ToolComputerUse,
        ToolTextEditor, ToolWebSearch,
    },
    Client,
};
use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, ProviderId},
        parameters::ParametersConfig,
    },
    providers::llm::anthropic::AnthropicConfig,
};
use jp_conversation::{
    message::Messages,
    thread::{Document, Documents, Thread},
    AssistantMessage, MessagePair, UserMessage,
};
use serde_json::Value;
use time::macros::date;
use tracing::{debug, info, trace, warn};

use super::{Event, EventStream, ModelDetails, Provider, ReasoningDetails, Reply, StreamEvent};
use crate::{
    error::{Error, Result},
    provider::{handle_delta, AccumulationState, Delta},
    query::ChatQuery,
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Anthropic;

/// Anthropic limits the number of cache points to 4 per request. Returning an API error if the
/// request exceeds this limit.
///
/// We detect where we inject cache controls and make sure to not exceed this limit.
const MAX_CACHE_CONTROL_COUNT: usize = 4;

#[derive(Debug, Clone)]
pub struct Anthropic {
    client: Client,
}

#[async_trait]
impl Provider for Anthropic {
    async fn models(&self) -> Result<Vec<ModelDetails>> {
        let (mut after_id, mut models) = self
            .client
            .models()
            .list()
            .await
            .map(|list| (list.has_more.then_some(list.last_id).flatten(), list.data))?;

        while let Some(id) = &after_id {
            let (id, data) = self
                .client
                .get::<ListModelsResponse>(&format!("/v1/models?after_id={id}"))
                .await
                .map(|list| (list.has_more.then_some(list.last_id).flatten(), list.data))?;

            models.extend(data);
            after_id = id;
        }

        Ok(models.into_iter().map(map_model).collect())
    }

    async fn chat_completion(
        &self,
        model: &ModelIdConfig,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<Reply> {
        let details = self.models().await?;
        let model_details = get_details_for_model(model, &details);
        let request = create_request(model, model_details, parameters, query, false)?;

        debug!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            stream = false,
            "Anthropic chat completion request."
        );

        self.client
            .messages()
            .create(request)
            .await
            .map_err(Into::into)
            .and_then(map_response)
            .map(|events| Reply {
                provider: PROVIDER,
                events,
            })
    }

    async fn chat_completion_stream(
        &self,
        model_id: &ModelIdConfig,
        parameters: &ParametersConfig,
        query: ChatQuery,
    ) -> Result<EventStream> {
        let client = self.client.clone();
        let details = self.models().await?;
        let model_details = get_details_for_model(model_id, &details);
        let request = create_request(model_id, model_details, parameters, query, true)?;

        debug!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            stream = true,
            "Anthropic chat completion stream request."
        );

        Ok(Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            let stream = client
                .messages()
                .create_stream(request).await
                .map_err(|e| match e {
                    AnthropicError::RateLimit { retry_after } =>
                        Error::RateLimit { retry_after: retry_after.map(Duration::from_secs) },

                    // Anthropic's API is notoriously unreliable, so we
                    // special-case the "overloaded" error, which is returned
                    // when their API is experiencing a high load.
                    //
                    // See: <https://docs.claude.com/en/docs/build-with-claude/streaming#error-events>
                    // See: <https://docs.claude.com/en/api/errors#http-errors>
                    AnthropicError::StreamError(e) if &e.error_type == "overloaded_error" =>
                        Error::RateLimit { retry_after: Some(Duration::from_secs(3)) },

                    _ => Error::from(e),
                });

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                for event in map_event(event?, &mut current_state) {
                    yield event;
                }
            }
        }))
    }
}

#[expect(clippy::too_many_lines)]
fn create_request(
    model_id: &ModelIdConfig,
    model_details: Option<&ModelDetails>,
    parameters: &ParametersConfig,
    query: ChatQuery,
    stream: bool,
) -> Result<types::CreateMessagesRequest> {
    let ChatQuery {
        thread,
        tools,
        mut tool_choice,
        tool_call_strict_mode,
    } = query;

    let mut builder = types::CreateMessagesRequestBuilder::default();

    builder.stream(stream);

    let Thread {
        system_prompt,
        instructions,
        attachments,
        history,
        message,
    } = thread;

    let mut cache_control_count = MAX_CACHE_CONTROL_COUNT;

    builder
        .model(model_id.name.clone())
        .messages(AnthropicMessages::build(history, message, &mut cache_control_count).0);

    let tools = convert_tools(tools, tool_call_strict_mode, &mut cache_control_count);

    let mut system_content = vec![];

    if let Some(text) = system_prompt {
        system_content.push(types::SystemContent::Text(types::Text {
            text,
            cache_control: (cache_control_count > 0).then_some({
                cache_control_count = cache_control_count.saturating_sub(1);
                types::CacheControl::default()
            }),
        }));
    }

    if !instructions.is_empty() {
        let text = instructions
            .into_iter()
            .map(|instruction| instruction.try_to_xml().map_err(Into::into))
            .collect::<Result<Vec<_>>>()?
            .join("\n\n");

        system_content.push(types::SystemContent::Text(types::Text {
            text,
            cache_control: (cache_control_count > 0).then_some({
                cache_control_count = cache_control_count.saturating_sub(1);
                types::CacheControl::default()
            }),
        }));
    }

    if !attachments.is_empty() {
        let documents: Documents = attachments
            .into_iter()
            .enumerate()
            .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
            .map(Document::from)
            .collect::<Vec<_>>()
            .into();

        system_content.push(types::SystemContent::Text(types::Text {
            text: documents.try_to_xml()?,

            // Anthropic limits the number of cache points to 4 per request. We
            // currently use 5 cache points, so this one is optional, depending
            // on whether we have any tools or not, making sure we stay within
            // the limit.
            cache_control: (cache_control_count > 0).then_some({
                _ = cache_control_count.saturating_sub(1);
                types::CacheControl::default()
            }),
        }));
    }

    let tool_choice_function = match tool_choice.clone() {
        ToolChoice::Function(name) => Some(name),
        _ => None,
    };

    // If there is only one tool, we can set the tool choice to "required",
    // since that gets us the same behavior, but avoids the issue of not
    // supporting reasoning when using the "function" tool choice.
    //
    // From testing, it seems that sending a single tool with the
    // "function" tool choice can result in incorrect API responses from
    // Anthropic. I (Jean) have an open support case with Anthropic to dig into
    // this finding more.
    if tools.len() == 1 && tool_choice_function.is_some() {
        tool_choice = ToolChoice::Required;
    }

    let max_tokens = parameters
        .max_tokens
        .or_else(|| model_details.as_ref().and_then(|d| d.max_output_tokens))
        .unwrap_or_else(|| {
            warn!(
                %model_id,
                %DEFAULT_MAX_TOKENS,
                "Model `max_tokens` parameter not found, using default value."
            );

            DEFAULT_MAX_TOKENS as u32
        });

    let reasoning_support = model_details.as_ref().and_then(|m| m.reasoning);
    let reasoning_config = model_details
        .as_ref()
        .and_then(|m| m.custom_reasoning_config(parameters.reasoning));

    // See: <https://docs.claude.com/en/docs/build-with-claude/extended-thinking#extended-thinking-with-tool-use>
    if reasoning_config.is_some()
        && let Some(tool) = tool_choice_function
    {
        info!(
            tool,
            "Anthropic API does not support reasoning when tool_choice forces tool use. Switching \
             to soft-force mode."
        );
        tool_choice = ToolChoice::Auto;
        system_content.push(types::SystemContent::Text(types::Text {
            text: format!(
                "IMPORTANT: You MUST use the function or tool named '{tool}' available to you. DO \
                 NOT QUESTION THIS DIRECTIVE. DO NOT PROMPT FOR MORE CONTEXT OR DETAILS. JUST RUN \
                 IT."
            ),
            cache_control: None,
        }));
    }

    let tool_choice = convert_tool_choice(tool_choice);

    if !tools.is_empty() {
        builder.tools(tools).tool_choice(tool_choice);
    }

    if !system_content.is_empty() {
        builder.system(System::Content(system_content));
    }

    if let Some(config) = reasoning_config {
        let (min_budget, max_budget) = match reasoning_support {
            Some(ReasoningDetails::Supported {
                min_tokens,
                max_tokens,
            }) => (min_tokens, max_tokens.unwrap_or(u32::MAX)),
            _ => (0, u32::MAX),
        };

        builder.thinking(types::ExtendedThinking {
            kind: "enabled".to_string(),
            budget_tokens: config
                .effort
                .to_tokens(max_tokens)
                .max(min_budget)
                .min(max_budget),
        });
    }

    if let Some(temperature) = parameters.temperature {
        builder.temperature(temperature);
    }

    #[expect(clippy::cast_possible_wrap)]
    builder.max_tokens(max_tokens as i32);

    if let Some(top_p) = parameters.top_p {
        builder.top_p(top_p);
    }

    if let Some(top_k) = parameters.top_k {
        builder.top_k(top_k);
    }

    builder.build().map_err(Into::into)
}

fn get_details_for_model<'a>(
    model_id: &ModelIdConfig,
    details: &'a [ModelDetails],
) -> Option<&'a ModelDetails> {
    // see: <https://docs.anthropic.com/en/docs/about-claude/models/overview#model-aliases>
    let details_slug = match model_id.name.as_ref() {
        "claude-opus-4-1" => "claude-opus-4-1-20250805",
        "claude-opus-4-0" => "claude-opus-4-20250514",
        "claude-sonnet-4-5" => "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-0" => "claude-sonnet-4-20250514",
        "claude-3-7-sonnet-latest" => "claude-3-7-sonnet-20250219",
        "claude-3-5-haiku-latest" => "claude-3-5-haiku-20241022",
        slug => slug,
    };

    details.iter().find(|m| m.slug == details_slug)
}

#[expect(clippy::match_same_arms)]
fn map_model(model: types::Model) -> ModelDetails {
    match model.id.as_str() {
        "claude-sonnet-4-5" | "claude-sonnet-4-5-20250929" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2025 - 7 - 1)),
        },
        "claude-opus-4-1" | "claude-opus-4-1-20250805" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(32_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
        },
        "claude-opus-4-0" | "claude-opus-4-20250514" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(32_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
        },
        "claude-sonnet-4-0" | "claude-sonnet-4-20250514" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            // TODO: The context window is 1_000_000 *IF* the
            // `context-1m-2025-08-07` beta header is set.
            //
            // We should probably update this method signature to take in the
            // final configuration, and change this value based on which header
            // is configured.
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2025 - 3 - 1)),
        },
        "claude-3-7-sonnet-latest" | "claude-3-7-sonnet-20250219" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::supported(0, None)),
            knowledge_cutoff: Some(date!(2024 - 11 - 1)),
        },
        "claude-3-5-haiku-latest" | "claude-3-5-haiku-20241022" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(8_192),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 7 - 1)),
        },
        "claude-3-5-sonnet-latest"
        | "claude-3-5-sonnet-20241022"
        | "claude-3-5-sonnet-20240620" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(8_192),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 4 - 1)),
        },
        "claude-3-opus-latest" | "claude-3-opus-20240229" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2023 - 8 - 1)),
        },
        "claude-3-haiku-20240307" => ModelDetails {
            provider: PROVIDER,
            slug: model.id,
            context_window: Some(200_000),
            max_output_tokens: Some(4_096),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(date!(2024 - 8 - 1)),
        },
        id => {
            warn!(model = id, ?model, "Missing model details.");

            ModelDetails {
                provider: PROVIDER,
                slug: model.id,
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            }
        }
    }
}

fn map_response(response: types::CreateMessagesResponse) -> Result<Vec<Event>> {
    debug!(
        response = serde_json::to_string(&response).unwrap_or_default(),
        "Received response from Anthropic API."
    );

    response
        .content
        .into_iter()
        .filter_map(|item| {
            let metadata = match &item {
                types::MessageContent::Thinking(Thinking { signature, .. }) => {
                    signature.clone().map(|v| ("signature", v))
                }
                types::MessageContent::RedactedThinking { data } => {
                    Some(("redacted_thinking", data.clone()))
                }
                _ => None,
            };

            let v: Option<Result<Event>> = Delta::from(item).into();
            v.map(|v| (v, metadata))
        })
        .flat_map(|item| match item {
            (v, _) if v.is_err() => vec![v],
            (v, None) => vec![v],
            (v, Some((key, value))) => vec![v, Ok(Event::metadata(key, value))],
        })
        .collect::<Result<_>>()
}

fn map_event(
    event: types::MessagesStreamEvent,
    state: &mut AccumulationState,
) -> Vec<Result<StreamEvent>> {
    use types::{ContentBlockDelta::*, MessagesStreamEvent::*};

    trace!(
        event = serde_json::to_string(&event).unwrap_or_default(),
        "Received event from Anthropic API."
    );

    match event {
        MessageStart { message, .. } => message
            .content
            .into_iter()
            .map(Delta::from)
            .filter_map(|v| handle_delta(v, state).transpose())
            .collect(),
        ContentBlockStart { content_block, .. } => handle_delta(content_block.into(), state)
            .transpose()
            .into_iter()
            .collect(),
        ContentBlockDelta { delta, .. } => match delta {
            TextDelta { text } => handle_delta(Delta::content(text), state)
                .transpose()
                .into_iter()
                .collect(),
            ThinkingDelta { thinking } => handle_delta(Delta::reasoning(thinking), state)
                .transpose()
                .into_iter()
                .collect(),
            InputJsonDelta { partial_json } => {
                handle_delta(Delta::tool_call("", "", partial_json), state)
                    .transpose()
                    .into_iter()
                    .collect()
            }

            // This is only used for thinking blocks, and we need to store this
            // signature to pass it back to the assistant in the message
            // history.
            //
            // See: <https://docs.anthropic.com/en/docs/build-with-claude/streaming#thinking-delta>
            SignatureDelta { signature } => vec![Ok(StreamEvent::metadata("signature", signature))],
        },
        MessageDelta { delta, .. }
            if delta.stop_reason.as_ref().is_some_and(|v| v == "tool_use") =>
        {
            handle_delta(Delta::tool_call_finished(), state)
                .transpose()
                .into_iter()
                .collect()
        }
        ContentBlockStop { .. } if state.is_accumulating() => {
            handle_delta(Delta::tool_call_finished(), state)
                .transpose()
                .into_iter()
                .collect()
        }
        _ => vec![],
    }
}

impl TryFrom<&AnthropicConfig> for Anthropic {
    type Error = Error;

    fn try_from(config: &AnthropicConfig) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        let mut builder = Client::builder();
        builder
            .api_key(api_key)
            .base_url(config.base_url.clone())
            .version("2023-06-01");

        if !config.beta_headers.is_empty() {
            builder.beta(config.beta_headers.join(","));
        }

        Ok(Anthropic {
            client: builder
                .build()
                .map_err(|e| Error::Anthropic(AnthropicError::Unknown(e.to_string())))?,
        })
    }
}

fn convert_tool_choice(choice: ToolChoice) -> types::ToolChoice {
    match choice {
        ToolChoice::None => types::ToolChoice::none(),
        ToolChoice::Auto => types::ToolChoice::auto(),
        ToolChoice::Required => types::ToolChoice::any(),
        ToolChoice::Function(name) => types::ToolChoice::tool(name),
    }
}

fn convert_tools(
    tools: Vec<ToolDefinition>,
    _strict: bool,
    cache_controls: &mut usize,
) -> Vec<types::Tool> {
    let mut tools: Vec<_> = tools
        .into_iter()
        .map(|tool| {
            types::Tool::Custom(types::CustomTool {
                name: tool.name,
                description: tool.description,
                input_schema: {
                    let required = tool
                        .parameters
                        .iter()
                        .filter(|(_, cfg)| cfg.required)
                        .map(|(key, _)| key.clone())
                        .collect();

                    let properties = tool
                        .parameters
                        .into_iter()
                        .map(|(key, cfg)| (key, cfg.to_json_schema()))
                        .collect();

                    types::ToolInputSchema {
                        kind: types::ToolInputSchemaKind::Object,
                        properties,
                        required,
                    }
                },
                cache_control: None,
            })
        })
        .collect();

    // Cache tool definitions, as they are unlikely to change.
    if *cache_controls > 0
        && let Some(tool) = tools.last_mut()
    {
        let cache_control = match tool {
            types::Tool::Custom(tool) => &mut tool.cache_control,
            types::Tool::Bash(ToolBash::Bash20241022(tool)) => &mut tool.cache_control,
            types::Tool::Bash(ToolBash::Bash20250124(tool)) => &mut tool.cache_control,
            types::Tool::CodeExecution(ToolCodeExecution::CodeExecution20250522(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::ComputerUse(ToolComputerUse::ComputerUse20241022(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::ComputerUse(ToolComputerUse::ComputerUse20250124(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20241022(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20250124(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::TextEditor(ToolTextEditor::TextEditor20250429(tool)) => {
                &mut tool.cache_control
            }
            types::Tool::WebSearch(ToolWebSearch::WebSearch20250305(tool)) => {
                &mut tool.cache_control
            }
        };

        *cache_control = Some(types::CacheControl::default());
        *cache_controls = cache_controls.saturating_sub(1);
    }

    tools
}

struct AnthropicMessages(Vec<types::Message>);

impl AnthropicMessages {
    fn build(history: Messages, message: UserMessage, cache_controls: &mut usize) -> Self {
        let mut items = vec![];

        // Historical messages.
        let mut history = history
            .into_iter()
            .flat_map(message_pair_to_messages)
            .collect::<Vec<_>>();

        // Make sure to add cache control to the last history message.
        if *cache_controls > 0
            && let Some(message) = history.last_mut().and_then(|m| m.content.0.last_mut())
        {
            *cache_controls = cache_controls.saturating_sub(1);

            match message {
                types::MessageContent::Text(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                types::MessageContent::ToolUse(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                types::MessageContent::ToolResult(m) => {
                    m.cache_control = Some(types::CacheControl::default());
                }
                _ => {}
            }
        }

        items.extend(history);

        // User query
        match message {
            UserMessage::Query(text) => {
                items.push(types::Message {
                    role: types::MessageRole::User,
                    content: types::MessageContentList(vec![types::MessageContent::Text(
                        text.into(),
                    )]),
                });
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| types::Message {
                    role: types::MessageRole::User,
                    content: types::MessageContentList(vec![types::MessageContent::ToolResult(
                        types::ToolResult {
                            tool_use_id: result.id,
                            content: Some(result.content),
                            is_error: result.error,
                            cache_control: None,
                        },
                    )]),
                }));
            }
        }

        Self(items)
    }
}

fn message_pair_to_messages(msg: MessagePair) -> Vec<types::Message> {
    let (user, assistant) = msg.split();

    vec![
        user_message_to_message(user),
        assistant_message_to_message(assistant),
    ]
}

fn user_message_to_message(user: UserMessage) -> types::Message {
    let list = match user {
        UserMessage::Query(query) => vec![types::MessageContent::Text(query.into())],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| {
                types::MessageContent::ToolResult(types::ToolResult {
                    tool_use_id: result.id,
                    content: Some(result.content),
                    is_error: result.error,
                    cache_control: None,
                })
            })
            .collect(),
    };

    types::Message {
        role: types::MessageRole::User,
        content: types::MessageContentList(list),
    }
}

fn assistant_message_to_message(assistant: AssistantMessage) -> types::Message {
    let AssistantMessage {
        provider,
        reasoning,
        content,
        tool_calls,
        metadata,
    } = assistant;

    let mut list = vec![];
    if let Some(data) = metadata
        .get("redacted_thinking")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        list.push(types::MessageContent::RedactedThinking { data });
    } else if let Some(thinking) = reasoning {
        if provider == PROVIDER {
            list.push(types::MessageContent::Thinking(Thinking {
                thinking,
                signature: metadata
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }));
        } else {
            list.push(types::MessageContent::Text(
                format!("<think>\n{thinking}\n</think>\n\n").into(),
            ));
        }
    }

    if let Some(text) = content {
        list.push(types::MessageContent::Text(text.into()));
    }

    for tool_call in tool_calls {
        list.push(types::MessageContent::ToolUse(types::ToolUse {
            id: tool_call.id,
            input: tool_call.arguments,
            name: tool_call.name,
            cache_control: None,
        }));
    }

    types::Message {
        role: types::MessageRole::Assistant,
        content: types::MessageContentList(list),
    }
}

impl From<types::MessageContent> for Delta {
    fn from(item: types::MessageContent) -> Self {
        match item {
            types::MessageContent::Text(text) => Delta::content(text.text),
            types::MessageContent::Thinking(thinking) => Delta::reasoning(thinking.thinking),
            types::MessageContent::RedactedThinking { .. } => Delta::reasoning(String::new()),
            types::MessageContent::ToolUse(tool_use) => {
                Delta::tool_call(tool_use.id, tool_use.name, match &tool_use.input {
                    Value::Object(map) if !map.is_empty() => tool_use.input.to_string(),
                    _ => String::new(),
                })
            }
            types::MessageContent::ToolResult(_) => {
                debug_assert!(false, "Unexpected message content: {item:?}");
                Delta::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use indexmap::IndexMap;
    use jp_config::{
        conversation::tool::{OneOrManyTypes, ToolParameterConfig, ToolParameterItemsConfig},
        model::parameters::{CustomReasoningConfig, ReasoningEffort},
        providers::llm::LlmProviderConfig,
    };
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr() -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new("https://api.anthropic.com", fixtures)
    }

    #[test(tokio::test)]
    async fn test_anthropic_models() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().anthropic;

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

                Anthropic::try_from(&config).unwrap().models().await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_chat_completion() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let mut config = LlmProviderConfig::default().anthropic;
        let model_id = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
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

                Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model_id, &ParametersConfig::default(), query)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_tool_call() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().anthropic;
        let model_id = "anthropic/claude-3-7-sonnet-latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            tool_choice: ToolChoice::Function("run_me".to_owned()),
            tools: vec![ToolDefinition {
                name: "run_me".to_owned(),
                description: None,
                parameters: IndexMap::from_iter([
                    ("foo".to_owned(), ToolParameterConfig {
                        kind: OneOrManyTypes::One("string".into()),
                        default: Some("foo".into()),
                        description: None,
                        required: false,
                        enumeration: vec![],
                        items: None,
                    }),
                    ("bar".to_owned(), ToolParameterConfig {
                        kind: OneOrManyTypes::Many(vec!["string".into(), "array".into()]),
                        default: None,
                        description: None,
                        required: true,
                        enumeration: vec!["foo".into(), vec!["foo", "bar"].into()],
                        items: Some(ToolParameterItemsConfig {
                            kind: "string".to_owned(),
                        }),
                    }),
                ]),
            }],
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

                Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model_id, &ParametersConfig::default(), query)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_anthropic_redacted_thinking(
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = LlmProviderConfig::default().anthropic;
        let model_id = "anthropic/claude-3-7-sonnet-latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                // See: <https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking#thinking-redaction>
                message: "ANTHROPIC_MAGIC_STRING_TRIGGER_REDACTED_THINKING_46C9A13E193C177646C7398A98432ECCCE4C1253D5E2D82641AC0E52CC2876CB".into(),
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

                let parameters = ParametersConfig {
                    reasoning: Some(
                        CustomReasoningConfig {
                            effort: ReasoningEffort::Medium,
                            exclude: false,
                        }
                        .into(),
                    ),
                    ..Default::default()
                };

                let events = Anthropic::try_from(&config)
                    .unwrap()
                    .chat_completion(&model_id, &parameters, query.clone())
                    .await
                    .unwrap()
                    .into_inner();

                assert!(events.iter().any(
                    |event| matches!(event, Event::Metadata(k, _) if k == "redacted_thinking")
                ));

                events
            },
        )
        .await
    }

    #[test]
    fn test_create_request() {
        let model_id = "anthropic/claude-3-5-haiku-latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let parameters = ParametersConfig {
            top_p: Some(1.0),
            top_k: Some(40),
            reasoning: Some(
                CustomReasoningConfig {
                    effort: ReasoningEffort::Medium,
                    exclude: false,
                }
                .into(),
            ),
            ..Default::default()
        };

        let model_details = map_model(types::Model {
            id: "claude-3-5-haiku-latest".to_owned(),
            display_name: String::new(),
            created_at: String::new(),
            model_type: String::new(),
        });

        let request = create_request(&model_id, Some(&model_details), &parameters, query, false);

        insta::assert_debug_snapshot!(request);
    }

    #[test]
    fn test_get_details_for_model() {
        let details = vec![
            ModelDetails {
                provider: PROVIDER,
                slug: "claude-opus-4-20250514".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
            ModelDetails {
                provider: PROVIDER,
                slug: "claude-sonnet-4-20250514".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
            ModelDetails {
                provider: PROVIDER,
                slug: "claude-3-7-sonnet-20250219".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
            ModelDetails {
                provider: PROVIDER,
                slug: "claude-3-5-haiku-20241022".to_owned(),
                context_window: None,
                max_output_tokens: None,
                reasoning: None,
                knowledge_cutoff: None,
            },
        ];

        let cases = vec![
            ("anthropic/claude-opus-4-0", Some(&details[0])),
            ("anthropic/claude-opus-4-20250514", Some(&details[0])),
            ("anthropic/claude-sonnet-4-0", Some(&details[1])),
            ("anthropic/claude-sonnet-4-20250514", Some(&details[1])),
            ("anthropic/claude-3-7-sonnet-latest", Some(&details[2])),
            ("anthropic/claude-3-7-sonnet-20250219", Some(&details[2])),
            ("anthropic/claude-3-5-haiku-latest", Some(&details[4])),
            ("anthropic/claude-3-5-haiku-20241022", Some(&details[4])),
            ("anthropic/nonexistent", None),
        ];

        for (model_id, expected) in cases {
            let actual = get_details_for_model(&model_id.parse().unwrap(), &details);
            assert_eq!(actual, expected);
        }
    }
}
