use std::{collections::HashMap, env};

use async_stream::stream;
use async_trait::async_trait;
use chrono::NaiveDate;
use futures::{StreamExt as _, TryStreamExt as _};
use gemini_client_rs::{GeminiClient, GeminiError, types};
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
use serde_json::{Map, Value};
use tracing::{debug, trace};

use super::{EventStream, Provider};
use crate::{
    StreamErrorKind,
    error::{Error, Result, StreamError, looks_like_quota_error},
    event::{Event, FinishReason},
    model::{ModelDeprecation, ModelDetails, ReasoningDetails},
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
        let (request, structured) = create_request(model, query)?;
        let slug = model.id.name.clone();

        debug!(stream = true, "Google chat completion stream request.");
        trace!(
            request = serde_json::to_string(&request).unwrap_or_default(),
            "Request payload."
        );

        Ok(call(client, request, slug, 0, structured))
    }
}

fn call(
    client: GeminiClient,
    request: types::GenerateContentRequest,
    model: Name,
    tries: usize,
    is_structured: bool,
) -> EventStream {
    Box::pin(stream! {
        let mut state = IndexMap::new();
        let stream = client
            .stream_content(&model, &request)
            .await
            .map_err(|e| StreamError::other(e.to_string()))?
            .map_err(StreamError::from);

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            for event in map_response(event?, &mut state, is_structured).map_err(|e| StreamError::other(e.to_string()))? {
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
                    let mut next_stream = call(client.clone(), request.clone(), model.clone(), tries + 1, is_structured);
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
fn create_request(
    model: &ModelDetails,
    query: ChatQuery,
) -> Result<(types::GenerateContentRequest, bool)> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
    } = query;

    let Thread {
        system_prompt,
        sections,
        attachments,
        events,
    } = thread;

    // Only use the schema if the very last event is a ChatRequest with one.
    let structured_schema = events
        .last()
        .and_then(|e| e.event.as_chat_request())
        .and_then(|req| req.schema.clone());
    let is_structured = structured_schema.is_some();

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
        convert_tool_choice(tool_choice)
    };

    let reasoning = model.custom_reasoning_config(parameters.reasoning);
    let supports_thinking = model.reasoning.is_some_and(|r| !r.is_unsupported());

    // Add thinking config if the model supports it.
    let thinking_config = if let Some(details) = model.reasoning.filter(|_| supports_thinking) {
        if let Some(config) = reasoning {
            // Reasoning is enabled — configure thinking accordingly.
            Some(types::ThinkingConfig {
                include_thoughts: !config.exclude,
                thinking_budget: if details.is_leveled() {
                    None
                } else {
                    // TODO: Once the `gemini` crate supports `-1` for "auto"
                    // thinking, use that here if `effort` is `Auto`.
                    //
                    // See: <https://ai.google.dev/gemini-api/docs/thinking#set-budget>
                    #[expect(clippy::cast_sign_loss)]
                    let tokens = config
                        .effort
                        .to_tokens(max_output_tokens.unwrap_or(32_000) as u32)
                        .min(details.max_tokens().unwrap_or(u32::MAX))
                        .max(details.min_tokens());
                    Some(tokens)
                },
                thinking_level: match details {
                    ReasoningDetails::Leveled {
                        xlow,
                        low,
                        medium,
                        high,
                        xhigh: _,
                    } => {
                        let level = config
                            .effort
                            .abs_to_rel(max_output_tokens.map(i32::cast_unsigned))
                            .unwrap_or(ReasoningEffort::Auto);

                        match level {
                            ReasoningEffort::None | ReasoningEffort::Xlow if xlow => {
                                Some(types::ThinkingLevel::Minimal)
                            }
                            ReasoningEffort::Low if low => Some(types::ThinkingLevel::Low),
                            ReasoningEffort::Medium if medium => Some(types::ThinkingLevel::Medium),
                            ReasoningEffort::High if high => Some(types::ThinkingLevel::High),

                            // Any other level is unsupported and treated as
                            // high (since the documentation specifies this is
                            // the default).
                            _ => Some(types::ThinkingLevel::High),
                        }
                    }
                    _ => None,
                },
            })
        } else if details.min_tokens() > 0 {
            // Model requires a minimum thinking budget — can't fully disable.
            Some(types::ThinkingConfig {
                include_thoughts: false,
                thinking_budget: Some(details.min_tokens()),
                thinking_level: None,
            })
        } else {
            // Reasoning is off — explicitly disable thinking.
            Some(types::ThinkingConfig {
                include_thoughts: false,
                thinking_budget: Some(0),
                thinking_level: None,
            })
        }
    } else {
        None
    };

    let parts = {
        let mut parts = vec![];
        if let Some(text) = system_prompt {
            parts.push(types::ContentData::Text(text));
        }

        for section in &sections {
            parts.push(types::ContentData::Text(section.render()));
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

    // Set structured output config on GenerationConfig when a schema is present.
    // We use `_responseJsonSchema` (the JSON Schema field) rather than
    // `responseSchema` (the OpenAPI Schema field) so that standard JSON
    // Schema properties like `additionalProperties` are accepted.
    //
    // The schema is transformed to rewrite unsupported properties (e.g.
    // `const` → `enum`) so constraints aren't silently dropped.
    let (response_mime_type, response_json_schema) = match structured_schema {
        Some(schema) => (
            Some("application/json".to_owned()),
            Some(Value::Object(transform_schema(schema))),
        ),
        None => (None, None),
    };

    Ok((
        types::GenerateContentRequest {
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
                response_mime_type,
                response_json_schema,
                ..Default::default()
            }),
        },
        is_structured,
    ))
}

/// Map a Gemini model to a `ModelDetails`.
///
/// See: <https://ai.google.dev/gemini-api/docs/models>
/// See: <https://ai.google.dev/gemini-api/docs/thinking#levels-budgets>
#[expect(clippy::too_many_lines)]
fn map_model(model: types::Model) -> ModelDetails {
    let name = model.base_model_id.as_str();
    let display_name = Some(model.display_name);
    let context_window = Some(model.input_token_limit);
    let max_output_tokens = Some(model.output_token_limit);
    let Ok(id) = (PROVIDER, model.base_model_id.as_str()).try_into() else {
        return ModelDetails::empty((PROVIDER, "unknown").try_into().unwrap());
    };

    match name {
        "gemini-pro-latest" | "gemini-3.1-pro-preview" | "gemini-3.1-pro-preview-customtools" => {
            ModelDetails {
                id,
                display_name,
                context_window,
                max_output_tokens,
                reasoning: Some(ReasoningDetails::leveled(false, true, true, true, false)),
                knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
                deprecated: Some(ModelDeprecation::Active),
                features: vec![],
            }
        }
        "gemini-3-pro-preview" => ModelDetails {
            id,
            display_name,
            context_window,
            max_output_tokens,
            reasoning: Some(ReasoningDetails::leveled(false, true, false, true, false)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gemini-flash-latest" | "gemini-3-flash-preview" => ModelDetails {
            id,
            display_name,
            context_window,
            max_output_tokens,
            reasoning: Some(ReasoningDetails::leveled(true, true, true, true, false)),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
            deprecated: Some(ModelDeprecation::Active),
            features: vec![],
        },
        "gemini-2.5-flash" => ModelDetails {
            id,
            display_name,
            context_window,
            max_output_tokens,
            reasoning: Some(ReasoningDetails::budgetted(0, Some(24576))),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gemini-3-flash-preview",
                Some(NaiveDate::from_ymd_opt(2026, 6, 17).unwrap()),
            )),
            features: vec![],
        },
        "gemini-flash-lite-latest"
        | "gemini-2.5-flash-lite"
        | "gemini-2.5-flash-lite-preview-09-2025" => ModelDetails {
            id,
            display_name,
            context_window,
            max_output_tokens,
            reasoning: Some(ReasoningDetails::budgetted(512, Some(24576))),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: unknown",
                Some(NaiveDate::from_ymd_opt(2026, 7, 22).unwrap()),
            )),
            features: vec![],
        },
        "gemini-2.5-pro" => ModelDetails {
            id,
            display_name,
            context_window,
            max_output_tokens,
            reasoning: Some(ReasoningDetails::budgetted(512, Some(24576))),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gemini-3-pro-preview",
                Some(NaiveDate::from_ymd_opt(2026, 6, 17).unwrap()),
            )),
            features: vec![],
        },
        "gemini-2.0-flash" | "gemini-2.0-flash-001" => ModelDetails {
            id,
            display_name,
            context_window,
            max_output_tokens,
            reasoning: Some(ReasoningDetails::budgetted(0, Some(24576))),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 8, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gemini-2.5-flash",
                Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()),
            )),
            features: vec![],
        },
        "gemini-2.0-flash-lite" | "gemini-2.0-flash-lite-001" => ModelDetails {
            id,
            display_name,
            context_window,
            max_output_tokens,
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: Some(NaiveDate::from_ymd_opt(2024, 8, 1).unwrap()),
            deprecated: Some(ModelDeprecation::deprecated(
                &"recommended replacement: gemini-2.5-flash-lite",
                Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()),
            )),
            features: vec![],
        },
        id => {
            trace!(
                name,
                display_name = display_name
                    .clone()
                    .unwrap_or_else(|| "<unknown>".to_owned()),
                id,
                "Missing model details. Falling back to generic model details."
            );

            ModelDetails {
                id: (PROVIDER, model.base_model_id.as_str())
                    .try_into()
                    .unwrap_or((PROVIDER, "unknown").try_into().unwrap()),
                display_name,
                context_window,
                max_output_tokens,
                reasoning: None,
                knowledge_cutoff: None,
                deprecated: None,
                features: vec![],
            }
        }
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
    is_structured: bool,
) -> Result<Vec<Event>> {
    debug!("Received response from Google API.");
    trace!(
        response = serde_json::to_string(&response).unwrap_or_default(),
        "Response payload."
    );

    response
        .candidates
        .into_iter()
        .flat_map(|v| map_candidate(v, state, is_structured))
        .try_fold(vec![], |mut acc, events| {
            acc.extend(events);
            Ok(acc)
        })
}

fn map_candidate(
    candidate: types::Candidate,
    states: &mut IndexMap<usize, CandidateState>,
    is_structured: bool,
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

        // Google sometimes sends empty text parts (e.g. the final chunk of a
        // thinking+tool_use response with finishReason: STOP). Skip them before
        // mode transition logic to avoid spurious flushes.
        if matches!(&data, types::ContentData::Text(text) if text.is_empty()) {
            continue;
        }

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
            types::ContentData::Text(text) if is_structured => Event::Part {
                event: ConversationEvent::now(ChatResponse::structured(Value::String(text))),
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

/// Transform a JSON schema to conform to Google's structured output constraints.
///
/// Google's Gemini API supports a subset of JSON Schema. This transformation:
/// - Inlines `$ref` references by replacing them with the referenced `$defs`
/// - Removes `$defs`/`definitions` from the output after inlining
/// - Rewrites `const` to `enum` with a single value (Google ignores `const`)
/// - Adds `propertyOrdering` to objects with multiple properties
/// - Recursively processes `anyOf`, object properties, `additionalProperties`,
///   array `items`, and `prefixItems`
///
/// Mirrors the logic from Google's Python SDK `process_schema`.
///
/// See: <https://ai.google.dev/gemini-api/docs/structured-output>
fn transform_schema(mut src: Map<String, Value>) -> Map<String, Value> {
    // Extract $defs from the root. They are inlined wherever $ref appears
    // and discarded from the output.
    let defs = src
        .remove("$defs")
        .or_else(|| src.remove("definitions"))
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();

    process_schema(src, &defs)
}

/// Core recursive processor for a single schema node.
fn process_schema(mut src: Map<String, Value>, defs: &Map<String, Value>) -> Map<String, Value> {
    // Resolve $ref by inlining the referenced definition.
    if let Some(Value::String(ref_path)) = src.remove("$ref")
        && let Some(resolved) = resolve_ref(&ref_path, defs)
    {
        let mut merged = resolved;
        // Preserve sibling fields (e.g. description, nullable) from the
        // referring schema — the definition's own fields take precedence.
        for (k, v) in src {
            merged.entry(k).or_insert(v);
        }
        return process_schema(merged, defs);
    }

    // Rewrite `const` to `enum` with a single-element array. Google supports
    // `enum` for strings and numbers but not `const`.
    if let Some(val) = src.remove("const") {
        src.insert("enum".into(), Value::Array(vec![val]));
    }

    // Handle anyOf: recurse into each variant, then return early (matching the
    // Python SDK's behavior).
    if let Some(Value::Array(variants)) = src.remove("anyOf") {
        src.insert(
            "anyOf".into(),
            Value::Array(
                variants
                    .into_iter()
                    .map(|v| resolve_and_process(v, defs))
                    .collect(),
            ),
        );
        return src;
    }

    // Type-specific processing.
    match src.get("type").and_then(Value::as_str) {
        Some("object") => {
            if let Some(Value::Object(props)) = src.remove("properties") {
                let keys: Vec<String> = props.keys().cloned().collect();
                let processed: Map<String, Value> = props
                    .into_iter()
                    .map(|(k, v)| (k, resolve_and_process(v, defs)))
                    .collect();

                // Deterministic output ordering for objects with >1 property.
                if keys.len() > 1 && !src.contains_key("propertyOrdering") {
                    src.insert(
                        "propertyOrdering".into(),
                        Value::Array(keys.into_iter().map(Value::String).collect()),
                    );
                }

                src.insert("properties".into(), Value::Object(processed));
            }

            // Process additionalProperties when it's a schema (not a boolean).
            if let Some(additional) = src.remove("additionalProperties") {
                src.insert("additionalProperties".into(), match additional {
                    Value::Object(schema) => Value::Object(process_schema(schema, defs)),
                    other => other,
                });
            }
        }
        Some("array") => {
            if let Some(items) = src.remove("items") {
                src.insert("items".into(), resolve_and_process(items, defs));
            }

            if let Some(Value::Array(prefixes)) = src.remove("prefixItems") {
                src.insert(
                    "prefixItems".into(),
                    Value::Array(
                        prefixes
                            .into_iter()
                            .map(|v| resolve_and_process(v, defs))
                            .collect(),
                    ),
                );
            }
        }
        _ => {}
    }

    src
}

/// Resolve any `$ref` inside a value, then recursively process it.
fn resolve_and_process(value: Value, defs: &Map<String, Value>) -> Value {
    match value {
        Value::Object(map) => Value::Object(process_schema(map, defs)),
        other => other,
    }
}

/// Look up a `$ref` path (e.g. `#/$defs/MyType`) in the definitions map.
fn resolve_ref(ref_path: &str, defs: &Map<String, Value>) -> Option<Map<String, Value>> {
    let name = ref_path.rsplit("defs/").next().unwrap_or(ref_path);
    defs.get(name).and_then(Value::as_object).cloned()
}

fn convert_tool_choice(choice: ToolChoice) -> types::ToolConfig {
    let (mode, allowed_function_names) = match choice {
        ToolChoice::None => (types::FunctionCallingMode::None, vec![]),
        ToolChoice::Auto => (types::FunctionCallingMode::Validated, vec![]),
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
                EventKind::ChatResponse(response) => {
                    let thought = response.is_reasoning();
                    let text = match response {
                        ChatResponse::Message { message } => message,
                        ChatResponse::Reasoning { reasoning } => reasoning,
                        ChatResponse::Structured { data } => data.to_string(),
                    };
                    (types::Role::Model, types::ContentPart {
                        thought,
                        data: types::ContentData::Text(text),
                        metadata: None,
                        thought_signature: None,
                    })
                }
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

impl From<GeminiError> for StreamError {
    fn from(err: GeminiError) -> Self {
        match err {
            GeminiError::Http(error) => Self::from(error),
            GeminiError::EventSource(error) => Self::from(error),
            GeminiError::Api(ref value) => {
                let msg = err.to_string();

                // Check for quota/billing exhaustion first.
                if looks_like_quota_error(&msg) {
                    return StreamError::new(
                        StreamErrorKind::InsufficientQuota,
                        format!(
                            "Insufficient API quota. Check your plan and billing details \
                             at https://console.cloud.google.com/billing. ({msg})"
                        ),
                    )
                    .with_source(err);
                }

                // Classify by HTTP status code if present in the API error.
                let status = value
                    .get("status")
                    .or_else(|| value.pointer("/error/code"))
                    .and_then(serde_json::Value::as_u64);

                match status {
                    Some(429) => StreamError::rate_limit(None).with_source(err),
                    Some(500 | 502 | 503 | 504) => StreamError::transient(msg).with_source(err),
                    _ => StreamError::other(msg).with_source(err),
                }
            }
            GeminiError::Json { data, error } => StreamError::other(data).with_source(error),
            GeminiError::FunctionExecution(msg) => StreamError::other(msg),
        }
    }
}

impl From<GeminiError> for Error {
    fn from(error: GeminiError) -> Self {
        match &error {
            GeminiError::Api(api) if api.get("status").is_some_and(|v| v.as_u64() == Some(404)) => {
                if let Some(model) = api.pointer("/message/error/message").and_then(|v| {
                    v.as_str().and_then(|s| {
                        s.contains("Call ListModels").then(|| {
                            s.split('/')
                                .nth(1)
                                .and_then(|v| v.split(' ').next())
                                .unwrap_or("unknown")
                        })
                    })
                }) {
                    return Self::UnknownModel(model.to_owned());
                }
                Self::Gemini(error)
            }
            _ => Self::Gemini(error),
        }
    }
}

#[cfg(test)]
#[path = "google_tests.rs"]
mod tests;
