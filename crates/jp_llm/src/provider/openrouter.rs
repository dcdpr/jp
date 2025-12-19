use std::env;

use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _, stream};
use indexmap::IndexMap;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ReasoningEffort,
    },
    providers::llm::openrouter::OpenrouterConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatResponse, EventKind},
    thread::{Document, Documents, Thread},
};
use jp_openrouter::{
    Client,
    types::{
        self,
        chat::{CacheControl, Content, Message},
        request::{self, RequestMessage},
        response::{
            self, ChatCompletion as OpenRouterChunk, FinishReason, ReasoningDetails,
            ReasoningDetailsFormat, ReasoningDetailsKind,
        },
        tool::{self, FunctionCall, Tool, ToolCall, ToolFunction},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tracing::{debug, trace, warn};

use super::{EventStream, ModelDetails};
use crate::{
    Error,
    error::Result,
    event::{self, Event},
    provider::{Provider, openai::parameters_with_strict_mode},
    query::ChatQuery,
    stream::aggregator::tool_call_request::ToolCallRequestAggregator,
};

static PROVIDER: ProviderId = ProviderId::Openrouter;

const ANTHROPIC_REDACTED_THINKING_KEY: &str = "anthropic_redacted_thinking";
const ANTHROPIC_THINKING_SIGNATURE_KEY: &str = "anthropic_thinking_signature";
const GOOGLE_THOUGHT_SIGNATURE_KEY: &str = "google_thought_signature";
const OPENAI_ENCRYPTED_CONTENT_KEY: &str = "openai_encrypted_content";

#[derive(Debug, Clone)]
pub struct Openrouter {
    client: Client,
}

impl Openrouter {
    fn new(api_key: String, app_name: Option<String>, app_referrer: Option<String>) -> Self {
        Self {
            client: Client::new(api_key, app_name, app_referrer),
        }
    }

    /// Set the base URL for the Openrouter API.
    fn with_base_url(mut self, base_url: String) -> Self {
        self.client = self.client.with_base_url(base_url);
        self
    }
}

#[async_trait]
impl Provider for Openrouter {
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
        let mut models = self
            .client
            .models()
            .await?
            .data
            .into_iter()
            .map(map_model)
            .collect::<Result<Vec<_>>>()?;

        models.sort_by(|a, b| a.id.cmp(&b.id));
        models.dedup();

        Ok(models)
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream> {
        debug!(
            model = %model.id,
            "Starting OpenRouter chat completion stream."
        );

        let mut state = AggregationState {
            tool_calls: ToolCallRequestAggregator::default(),
            aggregating_reasoning: false,
            aggregating_message: false,
        };

        let request = build_request(query, model)?;

        Ok(self
            .client
            .chat_completion_stream(request)
            .map_err(Error::from)
            .map_ok(move |v| stream::iter(map_completion(v, &mut state)))
            .try_flatten()
            .boxed())
    }
}

/// Aggregation state for a single stream of events.
struct AggregationState {
    /// Tool call aggregator.
    tool_calls: ToolCallRequestAggregator,

    /// Did the stream of events have any reasoning content?
    aggregating_reasoning: bool,

    /// Did the stream of events have any message content?
    aggregating_message: bool,
}

/// Metadata stored in the conversation stream, based on Openrouter
/// multi-provider support.
///
/// For example, if we use Openrouter to call an Openai model with reasoning
/// support, Openrouter will send us the "encryted reasoning" content in the
/// payload. We take that data, and morph it into a certain metadata shape that
/// can be read by both the Openrouter and Openai provider implementations, such
/// that the reasoning content can be used in future turns, regardless of
/// whether the conversation keeps using the Openrouter provider, or switches to
/// the Openai provider. The same applies to Anthropic, and other providers for
/// which Openrouter has provider-specific metadata support.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
struct MultiProviderMetadata {
    // NOTE: This has to remain in sync with
    // `crate::provider::openai::ENCODED_PAYLOAD_KEY`.
    //
    // If this proves difficult (here or in other fields), we will have to find
    // a working solution.
    #[serde(skip_serializing_if = "Option::is_none")]
    openai_encrypted_content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anthropic_thinking_signature: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anthropic_redacted_thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    google_thought_signature: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    openrouter_metadata: Vec<Map<String, Value>>,
}

impl MultiProviderMetadata {
    fn from_details(details: Vec<ReasoningDetails>) -> Self {
        let mut metadata = Self::default();

        for details in details {
            let ReasoningDetails {
                id,
                format,
                index,
                kind,
            } = details;

            let field = match (format, kind) {
                (Some(format), ReasoningDetailsKind::Encrypted { data }) => match format {
                    ReasoningDetailsFormat::OpenaiResponsesV1 => {
                        metadata.openai_encrypted_content = Some(data.into());
                        OPENAI_ENCRYPTED_CONTENT_KEY
                    }
                    ReasoningDetailsFormat::AnthropicClaudeV1 => {
                        metadata.anthropic_redacted_thinking = Some(data.into());
                        ANTHROPIC_REDACTED_THINKING_KEY
                    }
                    _ => "",
                },
                (
                    Some(format),
                    ReasoningDetailsKind::Text {
                        signature: Some(signature),
                        ..
                    },
                ) => match format {
                    ReasoningDetailsFormat::AnthropicClaudeV1 => {
                        metadata.anthropic_thinking_signature = Some(signature.into());
                        ANTHROPIC_THINKING_SIGNATURE_KEY
                    }
                    ReasoningDetailsFormat::GoogleGeminiV1 => {
                        metadata.google_thought_signature = Some(signature.into());
                        GOOGLE_THOUGHT_SIGNATURE_KEY
                    }
                    _ => "",
                },
                _ => "",
            };

            let mut map = Map::new();
            if !field.is_empty() {
                if let Some(id) = id {
                    map.insert("id".into(), id.into());
                }

                if let Some(index) = index {
                    map.insert("index".into(), index.into());
                }

                map.insert("field".into(), field.into());
            }
            if !map.is_empty() {
                metadata.openrouter_metadata.push(map);
            }
        }

        metadata
    }
}

impl From<MultiProviderMetadata> for IndexMap<String, Value> {
    fn from(val: MultiProviderMetadata) -> Self {
        let mut map = IndexMap::new();

        if let Some(v) = val.openai_encrypted_content {
            map.insert("openai_encrypted_content".into(), v);
        }

        if let Some(v) = val.anthropic_thinking_signature {
            map.insert("anthropic_thinking_signature".into(), v);
        }

        if let Some(v) = val.anthropic_redacted_thinking {
            map.insert("anthropic_redacted_thinking".into(), v);
        }

        if let Some(v) = val.google_thought_signature {
            map.insert("google_thought_signature".into(), v);
        }

        let metadata = val
            .openrouter_metadata
            .into_iter()
            .map(Value::from)
            .collect::<Vec<_>>();

        if !metadata.is_empty() {
            map.insert("openrouter_metadata".into(), metadata.into());
        }

        map
    }
}

fn map_completion(v: OpenRouterChunk, state: &mut AggregationState) -> Vec<Result<Event>> {
    v.choices
        .into_iter()
        .flat_map(|v| map_event(v, state))
        .collect()
}

#[expect(clippy::too_many_lines)]
fn map_event(choice: types::response::Choice, state: &mut AggregationState) -> Vec<Result<Event>> {
    let types::response::Choice::Streaming(types::response::StreamingChoice {
        finish_reason,
        delta:
            types::response::StreamingDelta {
                content,
                reasoning,
                tool_calls,
                reasoning_details,
                ..
            },
        error,
        ..
    }) = choice
    else {
        warn!("Received non-streaming choice in streaming context, ignoring.");
        return vec![];
    };

    // I _believe_ we can ignore the `reasoning.summary` details variant,
    // since it is basically a clone of the reasoning text we already have
    // in the regular `reasoning` field.
    let reasoning_details = reasoning_details
        .into_iter()
        .filter(|details| !matches!(details.kind, ReasoningDetailsKind::Summary { .. }))
        .collect::<Vec<_>>();

    let has_reasoning_details = !reasoning_details.is_empty();
    let reasoning_details = MultiProviderMetadata::from_details(reasoning_details);

    if let Some(error) = error {
        return vec![Err(Error::from(error))];
    }

    let mut events = vec![];
    let reasoning = reasoning.unwrap_or_default();
    if !reasoning.is_empty() || has_reasoning_details {
        state.aggregating_reasoning = true;

        let event = ConversationEvent::now(ChatResponse::reasoning(reasoning));
        let event = if has_reasoning_details {
            Ok(event.with_metadata(reasoning_details))
        } else {
            Ok(event)
        };

        events.push(event.map(|event| Event::Part { index: 0, event }));
    }

    if let Some(content) = content
        && !content.is_empty()
    {
        state.aggregating_message = true;

        events.push(Ok(Event::Part {
            index: 1,
            event: ConversationEvent::now(ChatResponse::message(content)),
        }));
    }

    if finish_reason.is_some() {
        if state.aggregating_reasoning {
            state.aggregating_reasoning = false;
            events.push(Ok(Event::flush(0)));
        }

        if state.aggregating_message {
            state.aggregating_message = false;
            events.push(Ok(Event::flush(1)));
        }
    }

    for (
        idx,
        types::tool::ToolCall::Function {
            function,
            id,
            index,
        },
    ) in tool_calls.into_iter().enumerate()
    {
        let index = idx + index + 2;
        state
            .tool_calls
            .add_chunk(index, id, function.name, function.arguments.as_deref());
    }

    if let Some(FinishReason::ToolCalls | FinishReason::Stop) = finish_reason {
        events.extend(
            state
                .tool_calls
                .finalize_all()
                .into_iter()
                .flat_map(|(index, result)| {
                    vec![
                        result
                            .map(|call| Event::Part {
                                index,
                                event: ConversationEvent::now(call),
                            })
                            .map_err(Error::from),
                        Ok(Event::flush(index)),
                    ]
                }),
        );
    }

    match finish_reason {
        Some(FinishReason::Length) => {
            events.push(Ok(Event::Finished(event::FinishReason::MaxTokens)));
        }
        Some(FinishReason::Stop) => {
            events.push(Ok(Event::Finished(event::FinishReason::Completed)));
        }
        Some(FinishReason::Error) => events.push(Err(jp_openrouter::Error::Stream(
            "unknown stream error".into(),
        )
        .into())),
        Some(reason) => events.push(Ok(Event::Finished(event::FinishReason::Other(
            reason.as_str().into(),
        )))),
        _ => {}
    }

    events
}

/// Build request for Openrouter API.
fn build_request(query: ChatQuery, model: &ModelDetails) -> Result<request::ChatCompletion> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let config = thread.events.config()?;
    let parameters = &config.assistant.model.parameters;

    let slug = model.id.name.to_string();
    let reasoning = model.custom_reasoning_config(parameters.reasoning);

    let messages: RequestMessages = (&model.id, thread).try_into()?;
    let tools = tools
        .into_iter()
        .map(|tool| Tool::Function {
            function: ToolFunction {
                parameters: parameters_with_strict_mode(tool.parameters, tool_call_strict_mode),
                name: tool.name,
                description: tool.description,
                strict: tool_call_strict_mode,
            },
        })
        .collect::<Vec<_>>();
    let tool_choice = if tools.is_empty() {
        None
    } else {
        Some(match tool_choice {
            ToolChoice::Auto => tool::ToolChoice::Auto,
            ToolChoice::None => tool::ToolChoice::None,
            ToolChoice::Required => tool::ToolChoice::Required,
            ToolChoice::Function(name) => tool::ToolChoice::function(name),
        })
    };

    trace!(
        slug,
        messages_size = messages.0.len(),
        tools_size = tools.len(),
        "Built Openrouter request."
    );

    Ok(request::ChatCompletion {
        model: slug,
        messages: messages.0,
        reasoning: reasoning.map(|r| request::Reasoning {
            exclude: r.exclude,
            effort: match r.effort.abs_to_rel(model.max_output_tokens) {
                ReasoningEffort::XHigh => request::ReasoningEffort::XHigh,
                ReasoningEffort::High => request::ReasoningEffort::High,
                ReasoningEffort::Auto | ReasoningEffort::Medium => request::ReasoningEffort::Medium,
                ReasoningEffort::Low => request::ReasoningEffort::Low,
                ReasoningEffort::Absolute(_) => {
                    debug_assert!(false, "Reasoning effort must be relative.");
                    request::ReasoningEffort::Medium
                }
            },
        }),
        tools,
        tool_choice,
        ..Default::default()
    })
}

// TODO: Manually add a bunch of often-used models.
fn map_model(model: response::Model) -> Result<ModelDetails> {
    Ok(ModelDetails {
        id: (PROVIDER, model.id).try_into()?,
        display_name: Some(model.name),
        context_window: Some(model.context_length),
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: Some(model.created.date()),
        deprecated: None,
        features: vec![],
    })
}

// impl From<StreamingDelta> for Delta {
//     fn from(delta: StreamingDelta) -> Self {
//         let tool_call = delta.tool_calls.into_iter().next();
//
//         Self {
//             content: delta.content,
//             reasoning: delta.reasoning,
//             tool_call_id: tool_call.as_ref().and_then(ToolCall::id),
//             tool_call_name: tool_call.as_ref().and_then(ToolCall::name),
//             tool_call_arguments: tool_call.as_ref().and_then(ToolCall::arguments),
//             tool_call_finished: false,
//         }
//     }
// }

impl From<types::response::ErrorResponse> for Error {
    fn from(error: types::response::ErrorResponse) -> Self {
        Self::OpenRouter(jp_openrouter::Error::Api {
            code: error.code,
            message: error.message,
        })
    }
}

impl TryFrom<&OpenrouterConfig> for Openrouter {
    type Error = Error;

    fn try_from(config: &OpenrouterConfig) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        let client = Openrouter::new(
            api_key,
            Some(config.app_name.clone()),
            config.app_referrer.clone(),
        )
        .with_base_url(config.base_url.clone());

        Ok(client)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct RequestMessages(pub Vec<RequestMessage>);

impl TryFrom<(&ModelIdConfig, Thread)> for RequestMessages {
    type Error = Error;

    fn try_from((model_id, thread): (&ModelIdConfig, Thread)) -> Result<Self> {
        let Thread {
            system_prompt,
            instructions,
            attachments,
            events,
        } = thread;

        let mut messages = vec![];

        // Build system prompt with instructions and attachments
        let mut content = vec![];

        // System message first, if any.
        //
        // Cached (1/4), as it's not expected to change.
        if let Some(system_prompt) = system_prompt {
            content.push(Content::Text {
                text: system_prompt,
                cache_control: Some(CacheControl::Ephemeral),
            });
        }

        if !instructions.is_empty() {
            content.push(Content::Text {
                text: "Before we continue, here are some contextual details that will help you \
                       generate a better response."
                    .to_string(),
                cache_control: None,
            });

            // Then instructions in XML tags.
            //
            // Cached (3/4), (for the last instruction), as it's not expected to
            // change.
            let mut instructions = instructions.iter().peekable();
            while let Some(instruction) = instructions.next() {
                content.push(Content::Text {
                    text: instruction.try_to_xml()?,
                    cache_control: instructions
                        .peek()
                        .map_or(Some(CacheControl::Ephemeral), |_| None),
                });
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

            content.push(Content::Text {
                text: documents.try_to_xml()?,
                cache_control: Some(CacheControl::Ephemeral),
            });
        }

        // Add system message if we have any system content
        if !content.is_empty() {
            messages.push(Message::default().with_content(content).system());
        }

        // Convert all events to messages
        let event_messages = convert_events(events);
        messages.extend(event_messages);

        // Only Anthropic and Google models support explicit caching.
        if !model_id.name.starts_with("anthropic") && !model_id.name.starts_with("google") {
            trace!(
                slug = %model_id.name,
                "Model does not support caching directives, disabling cache."
            );
            for m in &mut messages {
                m.content_mut().iter_mut().for_each(Content::disable_cache);
            }
        }

        Ok(RequestMessages(messages))
    }
}

/// Converts a single event into `OpenRouter` request messages.
fn convert_events(events: ConversationStream) -> Vec<RequestMessage> {
    events
        .into_iter()
        .flat_map(|event| match event.event.kind {
            EventKind::ChatRequest(request) => {
                vec![Message::default().with_text(request.content).user()]
            }
            EventKind::ChatResponse(response) => match response {
                ChatResponse::Message { message } => {
                    vec![Message::default().with_text(message).assistant()]
                }
                ChatResponse::Reasoning { reasoning, .. } => {
                    vec![Message::default().with_reasoning(reasoning).assistant()]
                }
            },
            EventKind::ToolCallRequest(request) => {
                let message = Message {
                    tool_calls: vec![ToolCall::Function {
                        id: Some(request.id.clone()),
                        index: 0,
                        function: FunctionCall {
                            name: Some(request.name),
                            arguments: if request.arguments.is_empty() {
                                None
                            } else {
                                serde_json::to_string(&request.arguments).ok()
                            },
                        },
                    }],
                    ..Default::default()
                };

                vec![message.assistant()]
            }
            EventKind::ToolCallResponse(response) => {
                let content = match response.result {
                    Ok(content) => content,
                    Err(error) => error,
                };

                vec![RequestMessage::Tool(tool::Message {
                    tool_call_id: response.id,
                    content,
                    name: None,
                })]
            }
            EventKind::InquiryRequest(_) | EventKind::InquiryResponse(_) => vec![],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use jp_config::providers::llm::LlmProviderConfig;
    use jp_test::{Result, function_name};

    use super::*;
    use crate::test::TestRequest;

    macro_rules! test_all_models {
        ($($fn:ident),* $(,)?) => {
            mod anthropic { use super::*; $(test_all_models!(func; $fn, "openrouter/anthropic/claude-haiku-4.5");)* }
            mod google    { use super::*; $(test_all_models!(func; $fn, "openrouter/google/gemini-2.5-flash");)* }
            mod xai       { use super::*; $(test_all_models!(func; $fn, "openrouter/x-ai/grok-code-fast-1");)* }
            mod minimax   { use super::*; $(test_all_models!(func; $fn, "openrouter/minimax/minimax-m2");)* }
        };
        (func; $fn:ident, $model:literal) => {
            paste::paste! {
                #[test_log::test(tokio::test)]
                async fn [< test_ $fn >]() -> Result {
                    $fn($model, &format!("{}_{}", $model.split('/').nth(1).unwrap(), function_name!())).await
                }
            }
        };
    }

    test_all_models![sub_provider_event_metadata];

    async fn sub_provider_event_metadata(model: &str, test_name: &str) -> Result {
        let requests = vec![
            TestRequest::chat(ProviderId::Openrouter)
                .model(model.parse().unwrap())
                .enable_reasoning()
                .chat_request("Test message"),
        ];

        run_test(test_name, requests).await?;

        Ok(())
    }

    async fn run_test(
        test_name: impl AsRef<str>,
        requests: impl IntoIterator<Item = TestRequest>,
    ) -> Result {
        crate::test::run_chat_completion(
            test_name,
            env!("CARGO_MANIFEST_DIR"),
            ProviderId::Openrouter,
            LlmProviderConfig::default(),
            requests.into_iter().collect(),
        )
        .await
    }
}
