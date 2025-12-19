use std::{collections::HashMap, env};

use async_stream::stream;
use async_trait::async_trait;
use futures::{StreamExt as _, TryStreamExt as _};
use gemini_client_rs::{GeminiClient, types};
use indexmap::IndexMap;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ReasoningEffort,
    },
    providers::llm::google::GoogleConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, ConversationEvent, EventKind, ToolCallRequest},
    thread::{Document, Documents, Thread},
};
use serde_json::Value;
use tracing::{debug, trace};

use super::{EventStream, Provider};
use crate::{
    error::{Error, Result},
    event::{Event, FinishReason},
    model::{ModelDetails, ReasoningDetails},
    query::ChatQuery,
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Google;

const THOUGHT_SIGNATURE_KEY: &str = "google_thought_signature";
const THOUGHT_SIGNATURE_DUMMY_VALUE: &str = "skip_thought_signature_validator";

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

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream> {
        let client = self.client.clone();
        let request = create_request(model, query)?;
        let slug = model.id.name.clone();

        debug!(stream = true, "Google chat completion stream request.");
        trace!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            "Request payload."
        );

        Ok(call(client, request, slug, 0))
    }
}

fn call(
    client: GeminiClient,
    request: types::GenerateContentRequest,
    model: Name,
    tries: usize,
) -> EventStream {
    Box::pin(stream! {
        let mut state = IndexMap::new();
        let stream = client
            .stream_content(&model, &request)
            .await?
            .map_err(Error::from);

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            for event in map_response(event?, &mut state)? {
                // Sometimes the API returns an "unexpected tool call" error, if
                // a previous turn had tools available but those were made
                // unavailable in follow-up turns. This is a known issue:
                //
                // > Gemini models occasionally fail when invoking a tool,
                // > returning an UNEXPECTED_TOOL_CALL error. A temporary
                // > workaround is to retry the request.
                //
                // source: <https://developer.watson-orchestrate.ibm.com/release/knownissues>
                //
                // We already set the tool calling to `none` to prevent the
                // model from trying to call tools when none are available, but
                // this is to no avail, as we still observe the same behavior.
                //
                // So as a last resort, we do three retries to force the model
                // to generate a proper response, before giving up.
                let should_retry = matches!(&event, Event::Finished(FinishReason::Other(Value::String(s))) if s == "UNEXPECTED_TOOL_CALL");

                if should_retry && tries < 3 {
                    let mut next_stream = call(client.clone(), request.clone(), model.clone(), tries + 1);
                    while let Some(item) = next_stream.next().await {
                      yield item;
                    }
                    return;
                }

                yield Ok(event);
            }
        }
    })
}

#[expect(clippy::too_many_lines)]
fn create_request(model: &ModelDetails, query: ChatQuery) -> Result<types::GenerateContentRequest> {
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
        events,
    } = thread;

    let config = events.config()?;
    let parameters = &config.assistant.model.parameters;

    let tools = convert_tools(tools);

    #[expect(clippy::cast_possible_wrap)]
    let max_output_tokens = parameters
        .max_tokens
        .or(model.max_output_tokens)
        .map(|v| v as i32);

    // We need to explicitly disallow any tool calls if there are no tools
    // available. This is because Gemini can "see" tool calls in its history and
    // try to call them again, even if they are not available.
    //
    // See also: <https://github.com/googleapis/python-genai/issues/1818>
    let tool_config = if tools.is_empty() {
        types::ToolConfig {
            function_calling_config: types::FunctionCallingConfig {
                mode: types::FunctionCallingMode::None,
                allowed_function_names: vec![],
            },
        }
    } else {
        convert_tool_choice(tool_choice, tool_call_strict_mode)
    };

    let reasoning = model.custom_reasoning_config(parameters.reasoning);

    // Add thinking config if the model requires it, or if it supports it,
    // and we have the parameters configured.
    let thinking_config = model
        .reasoning
        .filter(|details| (details.min_tokens() > 0) || reasoning.is_some())
        .map(|details| types::ThinkingConfig {
            include_thoughts: reasoning.is_some_and(|v| !v.exclude),
            thinking_budget: match details {
                ReasoningDetails::Leveled { .. } => None,
                _ => reasoning.map(|v| {
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
            },
            thinking_level: match details {
                ReasoningDetails::Leveled { low, high, .. } => {
                    let level = reasoning.map(|v| {
                        v.effort
                            .abs_to_rel(max_output_tokens.map(i32::cast_unsigned))
                    });

                    match level {
                        Some(ReasoningEffort::Low) if low => Some(types::ThinkingLevel::Low),
                        Some(ReasoningEffort::High) if high => Some(types::ThinkingLevel::High),
                        // Any other level is unsupported and treated as
                        // high (since the documentation specifies this is
                        // the default).
                        _ => Some(types::ThinkingLevel::High),
                    }
                }
                _ => None,
            },
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
                thought_signature: None,
            })
            .collect::<Vec<_>>()
    };

    Ok(types::GenerateContentRequest {
        system_instruction: if parts.is_empty() {
            None
        } else {
            Some(types::Content { parts, role: None })
        },
        contents: convert_events(events),
        tools,
        tool_config: Some(tool_config),
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
            .then_some(ReasoningDetails::budgetted(128, Some(32768)))
            .or_else(|| {
                model
                    .base_model_id
                    .starts_with("gemini-2.5-flash")
                    .then_some(ReasoningDetails::budgetted(0, Some(24576)))
            })
            .or_else(|| {
                model
                    .base_model_id
                    .starts_with("gemini-3")
                    .then_some(ReasoningDetails::leveled(true, false, true, false))
            }),
        knowledge_cutoff: None,
        deprecated: None,
        features: vec![],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentMode {
    Reasoning,
    Message,
    FunctionCall,
}

impl ContentMode {
    fn is_reasoning(self) -> bool {
        matches!(self, Self::Reasoning)
    }
}

struct CandidateState {
    current_virtual_index: usize,
    last_mode: Option<ContentMode>,
}

impl CandidateState {
    fn new(base_index: usize) -> Self {
        Self {
            current_virtual_index: base_index * 1000,
            last_mode: None,
        }
    }
}

fn map_response(
    response: types::GenerateContentResponse,
    state: &mut IndexMap<usize, CandidateState>,
) -> Result<Vec<Event>> {
    debug!("Received response from Google API.");
    trace!(
        response = serde_json::to_string(&response).unwrap_or_default(),
        "Response payload."
    );

    response
        .candidates
        .into_iter()
        .flat_map(|v| map_candidate(v, state))
        .try_fold(vec![], |mut acc, events| {
            acc.extend(events);
            Ok(acc)
        })
}

fn map_candidate(
    candidate: types::Candidate,
    states: &mut IndexMap<usize, CandidateState>,
) -> Result<Vec<Event>> {
    let types::Candidate {
        content,
        finish_reason,
        index,
        ..
    } = candidate;

    let mut events = Vec::new();
    let index = index.unwrap_or_default() as usize;
    let state = states
        .entry(index)
        .or_insert_with(|| CandidateState::new(index));

    for part in content.into_iter().flat_map(|v| v.parts) {
        let types::ContentPart {
            thought,
            data,
            thought_signature,
            ..
        } = part;

        // Determine what "mode" the content is in.
        let mode = if matches!(data, types::ContentData::FunctionCall(_)) {
            ContentMode::FunctionCall
        } else if thought {
            ContentMode::Reasoning
        } else {
            ContentMode::Message
        };

        // If we change from one mode to another, flush the current index, and
        // increment the current (virtual) index by one.
        //
        // We also increment the virtual index if we're in a function call, as
        // function calls are always standalone events.
        if state
            .last_mode
            .is_some_and(|v| v != mode || v == ContentMode::FunctionCall)
        {
            events.push(Event::flush(state.current_virtual_index));

            state.current_virtual_index += 1;
        }

        // Store the current mode for the next iteration.
        state.last_mode = Some(mode);

        let index = state.current_virtual_index;
        let mut event = match data {
            types::ContentData::Text(text) if mode.is_reasoning() => Event::Part {
                event: ConversationEvent::now(ChatResponse::reasoning(text)),
                index,
            },
            types::ContentData::Text(text) => Event::Part {
                event: ConversationEvent::now(ChatResponse::message(text)),
                index,
            },
            types::ContentData::FunctionCall(types::FunctionCall {
                id,
                name,
                arguments,
            }) => Event::Part {
                event: ConversationEvent::now(ToolCallRequest {
                    id: id.unwrap_or_else(|| format!("{name}_{index}")),
                    name,
                    arguments: match arguments {
                        serde_json::Value::Object(map) => map,
                        v => serde_json::Map::from_iter([("input".into(), v)]),
                    },
                }),
                index,
            },
            _ => continue,
        };

        if let Some(v) = thought_signature
            && let Event::Part { event, .. } = &mut event
        {
            event.add_metadata_field(THOUGHT_SIGNATURE_KEY, v);
        }

        events.push(event);
    }

    if let Some(reason) = finish_reason {
        // For `MaxTokens`, we do not flush any indices, as we assume the active
        // indices aren't complete yet. The caller can still decide to flush
        // manually.
        if matches!(reason, types::FinishReason::MaxTokens) {
            events.push(Event::Finished(FinishReason::MaxTokens));
            return Ok(events);
        }

        events.extend(
            states
                .values()
                .map(|s| Event::flush(s.current_virtual_index)),
        );

        match reason {
            types::FinishReason::Stop => {
                events.push(Event::Finished(FinishReason::Completed));
            }
            v => {
                events.push(Event::Finished(FinishReason::Other(serde_json::to_value(
                    &v,
                )?)));
            }
        }
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

fn convert_tool_choice(choice: ToolChoice, strict: bool) -> types::ToolConfig {
    let (mode, allowed_function_names) = match choice {
        ToolChoice::None => (types::FunctionCallingMode::None, vec![]),
        ToolChoice::Auto if strict => (types::FunctionCallingMode::Validated, vec![]),
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

fn convert_tools(tools: Vec<ToolDefinition>) -> Vec<types::Tool> {
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

fn convert_events(events: ConversationStream) -> Vec<types::Content> {
    // Google requires the `ToolCallResponse` to contain the name of the tool
    // call from the `ToolCallRequest`, even though they also share the same ID.
    //
    // We don't store tool call names in `ToolCallResponse`, so we have to track
    // that here by storing the names of `ToolCallRequest`s, keyed by IDs, and
    // then using them for `ToolCallResponse`s with the same ID.
    //
    // This assumes that the invariant holds that a request always precedes its
    // response, but if that is untrue, we silently proceed without erroring.
    let mut tool_call_names = HashMap::new();

    events
        .into_iter()
        .filter_map(|event| {
            let ConversationEvent {
                kind, mut metadata, ..
            } = event.event;

            let (role, mut part) = match kind {
                EventKind::ChatRequest(request) => (
                    types::Role::User,
                    types::ContentData::Text(request.content).into(),
                ),
                EventKind::ChatResponse(response) => (types::Role::Model, types::ContentPart {
                    thought: response.is_reasoning(),
                    data: types::ContentData::Text(response.into_content()),
                    metadata: None,
                    thought_signature: None,
                }),
                EventKind::ToolCallRequest(request) => (types::Role::Model, types::ContentPart {
                    data: types::ContentData::FunctionCall(types::FunctionCall {
                        name: {
                            tool_call_names.insert(request.id.clone(), request.name.clone());
                            request.name
                        },
                        id: Some(request.id),
                        arguments: Value::Object(request.arguments),
                    }),

                    thought_signature: Some(
                        metadata
                            .shift_remove(THOUGHT_SIGNATURE_KEY)
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| THOUGHT_SIGNATURE_DUMMY_VALUE.to_owned()),
                    ),
                    thought: false,
                    metadata: None,
                }),
                EventKind::ToolCallResponse(response) => (
                    types::Role::User,
                    types::ContentData::FunctionResponse(types::FunctionResponse {
                        name: tool_call_names.remove(&response.id).unwrap_or_default(),
                        id: Some(response.id),
                        response: types::FunctionResponsePayload {
                            content: match response.result {
                                Ok(content) => Value::String(content),
                                Err(error) => Value::String(error),
                            },
                        },
                    })
                    .into(),
                ),
                _ => return None,
            };

            if part.thought_signature.is_none() {
                part.thought_signature = metadata
                    .shift_remove(THOUGHT_SIGNATURE_KEY)
                    .and_then(|v| v.as_str().map(str::to_owned));
            }

            Some((role, part))
        })
        .fold(vec![], |mut messages, (role, part)| {
            match messages.last_mut() {
                // If the last message has the same role, append part to it
                Some(last) if last.role == Some(role) => last.parts.push(part),
                // Different role or no messages yet, start a new message
                _ => messages.push(types::Content {
                    role: Some(role),
                    parts: vec![part],
                }),
            }

            messages
        })
}

#[cfg(test)]
mod tests {
    use jp_config::model::parameters::{
        PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningEffort,
    };
    use jp_conversation::event::ChatRequest;
    use jp_test::function_name;
    use test_log::test;

    use super::*;
    use crate::test::{TestRequest, run_test};

    // TODO: Test specific conditions as detailed in
    // <https://ai.google.dev/gemini-api/docs/thought-signatures>:
    //
    // - parallel function calls
    // - dummy thought signatures
    // - multi-turn conversations
    #[test(tokio::test)]
    async fn test_gemini_3_reasoning() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let request = TestRequest::chat(PROVIDER)
            .stream(true)
            .reasoning(Some(PartialReasoningConfig::Custom(
                PartialCustomReasoningConfig {
                    effort: Some(ReasoningEffort::Low),
                    exclude: Some(false),
                },
            )))
            .model("google/gemini-3-pro-preview".parse().unwrap())
            .event(ChatRequest::from("Test message"));

        run_test(PROVIDER, function_name!(), Some(request)).await
    }
}
