use std::env;

use async_trait::async_trait;
use futures::{StreamExt as _, future, stream};
use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::{ReasoningConfig, ReasoningEffort},
    },
    providers::llm::cerebras::CerebrasConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, EventKind, ToolCallResponse},
    thread::text_attachments_to_xml,
};
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest_eventsource::{Event as SseEvent, EventSource};
use serde_json::{Map, Value, json};
use tracing::{debug, trace, warn};

use super::{
    EventStream, ModelDetails, Provider, llamacpp::StreamChunk, openai::parameters_with_strict_mode,
};
use crate::{
    error::{Error, Result, StreamError},
    event::{Event, FinishReason},
    model::ReasoningDetails,
    query::ChatQuery,
    tool::ToolDefinition,
};

static PROVIDER: ProviderId = ProviderId::Cerebras;

#[derive(Debug, Clone)]
pub struct Cerebras {
    client: reqwest::Client,
    base_url: String,
}

#[async_trait]
impl Provider for Cerebras {
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
        let response: ModelsResponse = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut models: Vec<ModelDetails> = response
            .data
            .into_iter()
            .map(|m| map_model(&m.id))
            .collect::<Result<_>>()?;

        models.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(models)
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream> {
        debug!(
            model = %model.id.name,
            "Starting Cerebras chat completion stream."
        );

        let (body, is_structured) = build_request(model, query)?;

        trace!(
            body = serde_json::to_string(&body).unwrap_or_default(),
            "Sending request to Cerebras."
        );

        let request = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("content-type", "application/json")
            .json(&body);

        let es = EventSource::new(request).map_err(|e| Error::InvalidResponse(e.to_string()))?;

        let mut state = StreamState {
            tool_call_indices: Vec::new(),
            reasoning_flushed: false,
            finish_reason: None,
            is_structured,
        };

        Ok(es
            .take_while(|event| future::ready(event.is_ok()))
            .then(move |event| {
                let result = handle_sse_event_sync(event, &mut state);
                async move {
                    match result {
                        Ok(v) => stream::iter(v).boxed(),
                        Err(e) => {
                            stream::iter(vec![Err(StreamError::from_eventsource(e).await)]).boxed()
                        }
                    }
                }
            })
            .flatten()
            .boxed())
    }
}

impl TryFrom<&CerebrasConfig> for Cerebras {
    type Error = Error;

    fn try_from(config: &CerebrasConfig) -> Result<Self> {
        let api_key = env::var(&config.api_key_env)
            .map_err(|_| Error::MissingEnv(config.api_key_env.clone()))?;

        let client = reqwest::Client::builder()
            .default_headers(HeaderMap::from_iter([(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}"))
                    .map_err(|_| Error::InvalidResponse("invalid API key".into()))?,
            )]))
            .build()?;

        Ok(Cerebras {
            client,
            base_url: config.base_url.clone(),
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelEntry {
    id: String,
}

fn map_model(id: &str) -> Result<ModelDetails> {
    let details = match id {
        // Context and output limits use paid-tier values. Free-tier users
        // get lower limits enforced server-side.
        "llama3.1-8b" => ModelDetails {
            id: (PROVIDER, id).try_into()?,
            display_name: Some("Llama 3.1 8B".to_owned()),
            context_window: Some(32_768),
            max_output_tokens: Some(8_192),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: None,
            deprecated: None,
            structured_output: Some(true),
            features: vec![],
        },
        "qwen-3-235b-a22b-instruct-2507" => ModelDetails {
            id: (PROVIDER, id).try_into()?,
            display_name: Some("Qwen 3 235B A22B".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(40_960),
            reasoning: Some(ReasoningDetails::unsupported()),
            knowledge_cutoff: None,
            deprecated: None,
            structured_output: Some(true),
            features: vec![],
        },
        "gpt-oss-120b" => ModelDetails {
            id: (PROVIDER, id).try_into()?,
            display_name: Some("GPT-OSS 120B".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(40_960),
            reasoning: Some(ReasoningDetails::leveled(
                false, false, true, true, true, false,
            )),
            knowledge_cutoff: None,
            deprecated: None,
            structured_output: Some(true),
            features: vec![],
        },
        "zai-glm-4.7" => ModelDetails {
            id: (PROVIDER, id).try_into()?,
            display_name: Some("Zai GLM 4.7".to_owned()),
            context_window: Some(131_072),
            max_output_tokens: Some(40_960),
            // Reasoning is enabled by default; only `none` disables it.
            reasoning: Some(ReasoningDetails::leveled(
                true, false, false, false, false, false,
            )),
            knowledge_cutoff: None,
            deprecated: None,
            structured_output: Some(true),
            features: vec![],
        },
        _ => {
            warn!(model = id, "Unknown Cerebras model, using empty details.");
            ModelDetails::empty((PROVIDER, id).try_into()?)
        }
    };

    Ok(details)
}

fn build_request(model: &ModelDetails, query: ChatQuery) -> Result<(Value, bool)> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
    } = query;

    let structured_schema = thread.events.schema();
    let is_structured = structured_schema.is_some();

    let config = thread.events.config()?;
    let parameters = &config.assistant.model.parameters;
    let slug = model.id.name.to_string();

    let parts = thread.into_parts();

    let mut system_parts = parts.system_parts;
    if let Some(xml) = text_attachments_to_xml(&parts.attachments)? {
        system_parts.push(xml);
    }

    let mut messages: Vec<Value> = system_parts
        .into_iter()
        .map(|content| json!({ "role": "system", "content": content }))
        .collect();

    messages.extend(convert_events(parts.events));

    let converted_tools = convert_tools(tools);
    let tool_choice_val = convert_tool_choice(&tool_choice);

    trace!(
        slug,
        messages_size = messages.len(),
        tools_size = converted_tools.len(),
        "Built Cerebras request."
    );

    let reasoning_enabled = model.reasoning.is_some_and(|r| !r.is_unsupported());

    let mut body = json!({
        "model": slug,
        "messages": messages,
        "stream": true,
    });

    // Request parsed reasoning format so reasoning arrives in a separate
    // `reasoning` field rather than mixed into `content` with <think> tags.
    if reasoning_enabled {
        body["reasoning_format"] = json!("parsed");
    }

    if let Some(temperature) = parameters.temperature {
        body["temperature"] = json!(temperature);
    }

    if let Some(top_p) = parameters.top_p {
        body["top_p"] = json!(top_p);
    }

    if let Some(max_tokens) = parameters.max_tokens {
        body["max_completion_tokens"] = json!(max_tokens);
    }

    // Reasoning effort for gpt-oss-120b and zai-glm-4.7.
    let reasoning = model.custom_reasoning_config(parameters.reasoning);
    if let Some(r) = &reasoning {
        let effort_str = match r.effort {
            ReasoningEffort::Low | ReasoningEffort::Xlow | ReasoningEffort::None => "low",
            ReasoningEffort::Medium | ReasoningEffort::Auto | ReasoningEffort::Absolute(_) => {
                "medium"
            }
            ReasoningEffort::High | ReasoningEffort::XHigh | ReasoningEffort::Max => "high",
        };
        body["reasoning_effort"] = json!(effort_str);
    } else if matches!(parameters.reasoning, Some(ReasoningConfig::Off))
        && model
            .reasoning
            .and_then(|r| r.lowest_effort())
            .is_some_and(|e| e == ReasoningEffort::None)
    {
        // Only models that explicitly support `none` (e.g. zai-glm-4.7) can
        // have reasoning disabled this way. For others (e.g. gpt-oss-120b), we
        // simply omit reasoning_effort and let the server use its default.
        body["reasoning_effort"] = json!("none");
    }

    if !converted_tools.is_empty() {
        body["tools"] = json!(converted_tools);
        body["tool_choice"] = json!(tool_choice_val);
    }

    if let Some(schema) = structured_schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": "structured_output",
                "schema": transform_schema(schema),
                "strict": true,
            },
        });
    }

    Ok((body, is_structured))
}

/// Transform a JSON schema for Cerebras's strict structured output mode.
///
/// Cerebras requires `additionalProperties: false` on all objects and all
/// properties listed in `required`. It does not support `minItems`,
/// `maxItems`, string `pattern`, or string `format`.
///
/// See: <https://inference-docs.cerebras.ai/capabilities/structured-outputs#supported-schemas>
fn transform_schema(src: Map<String, Value>) -> Value {
    Value::Object(process_schema(src))
}

/// Build a clean output map from `src`, moving only supported fields and
/// recursing into nested schemas. Anything left in `src` after extraction
/// is unsupported and gets appended to the `description` as a soft hint.
fn process_schema(mut src: Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();

    macro_rules! move_field {
        ($key:literal) => {
            if let Some(v) = src.remove($key) {
                out.insert($key.into(), v);
            }
        };
    }

    // Common fields.
    move_field!("title");
    move_field!("description");
    move_field!("const");
    move_field!("enum");
    move_field!("default");
    move_field!("$ref");

    // Recursive definitions.
    for key in ["$defs", "definitions"] {
        if let Some(Value::Object(defs)) = src.remove(key) {
            let processed: Map<String, Value> = defs
                .into_iter()
                .map(|(k, v)| (k, process_value(v)))
                .collect();
            out.insert(key.into(), Value::Object(processed));
        }
    }

    // Combinators.
    if let Some(Value::Array(variants)) = src.remove("anyOf") {
        out.insert(
            "anyOf".into(),
            Value::Array(variants.into_iter().map(process_value).collect()),
        );
    }

    // Type-specific handling. Remove `type` from src so it doesn't end up
    // in the leftovers.
    let type_val = src.remove("type");
    match type_val.as_ref().and_then(Value::as_str) {
        Some("object") => {
            if let Some(Value::Object(props)) = src.remove("properties") {
                let processed: Map<String, Value> = props
                    .into_iter()
                    .map(|(k, v)| (k, process_value(v)))
                    .collect();

                // All properties must be in `required` for strict mode.
                let keys: Vec<_> = processed.keys().map(|k| Value::String(k.clone())).collect();
                out.insert("required".into(), Value::Array(keys));
                out.insert("properties".into(), Value::Object(processed));
            }

            src.remove("required");
            src.remove("additionalProperties");
            out.insert("additionalProperties".into(), Value::Bool(false));
        }
        Some("array") => {
            if let Some(items) = src.remove("items") {
                out.insert("items".into(), process_value(items));
            }

            // Number constraints on arrays are unsupported — leave them in
            // `src` so they fall through to the description hint.
        }
        // `string`: `pattern` and `format` are unsupported — left in `src`
        // so they fall through to the description hint.
        Some("number" | "integer") => {
            // Number constraints are supported.
            move_field!("minimum");
            move_field!("maximum");
            move_field!("exclusiveMinimum");
            move_field!("exclusiveMaximum");
            move_field!("multipleOf");
        }
        _ => {}
    }

    if let Some(t) = type_val {
        out.insert("type".into(), t);
    }

    // Anything still in `src` is unsupported. Append to description so the
    // model sees it as a soft hint.
    if !src.is_empty() {
        let extra = src
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(", ");

        out.entry("description")
            .and_modify(|v| {
                if let Some(s) = v.as_str() {
                    *v = Value::from(format!("{s}\n\n{{{extra}}}"));
                }
            })
            .or_insert_with(|| Value::from(format!("{{{extra}}}")));
    }

    out
}

fn process_value(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(process_schema(map)),
        other => other,
    }
}

fn convert_events(events: ConversationStream) -> Vec<Value> {
    events
        .into_iter()
        .filter_map(|event| match event.into_kind() {
            EventKind::ChatRequest(request) => {
                Some(json!({ "role": "user", "content": request.content }))
            }
            EventKind::ChatResponse(response) => match response {
                ChatResponse::Message { message } => {
                    Some(json!({ "role": "assistant", "content": message }))
                }
                ChatResponse::Reasoning { reasoning } => Some(json!({
                    "role": "assistant",
                    "reasoning": reasoning,
                })),
                ChatResponse::Structured { data } => {
                    Some(json!({ "role": "assistant", "content": data.to_string() }))
                }
            },
            EventKind::ToolCallRequest(request) => Some(json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": request.id,
                    "type": "function",
                    "function": {
                        "name": request.name,
                        "arguments": Value::Object(request.arguments).to_string(),
                    },
                }],
            })),
            EventKind::ToolCallResponse(ToolCallResponse { id, result }) => Some(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": match result {
                    Ok(content) | Err(content) => content,
                },
            })),
            _ => None,
        })
        .collect()
}

fn convert_tools(tools: Vec<ToolDefinition>) -> Vec<Value> {
    tools
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.docs.schema_description().unwrap_or_default(),
                    "parameters": parameters_with_strict_mode(tool.parameters, false),
                },
            })
        })
        .collect()
}

fn convert_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Function(name) => json!({
            "type": "function",
            "function": { "name": name },
        }),
    }
}

struct StreamState {
    tool_call_indices: Vec<usize>,
    reasoning_flushed: bool,
    finish_reason: Option<FinishReason>,
    is_structured: bool,
}

type SseResult =
    std::result::Result<Vec<std::result::Result<Event, StreamError>>, reqwest_eventsource::Error>;

fn handle_sse_event_sync(
    event: std::result::Result<SseEvent, reqwest_eventsource::Error>,
    state: &mut StreamState,
) -> SseResult {
    match event {
        Ok(SseEvent::Open) => Ok(vec![]),
        Ok(SseEvent::Message(msg)) => {
            if msg.data == "[DONE]" {
                let mut events: Vec<std::result::Result<Event, StreamError>> = vec![];

                if !state.reasoning_flushed {
                    events.push(Ok(Event::flush(0)));
                    state.reasoning_flushed = true;
                }
                events.push(Ok(Event::flush(1)));
                events.push(Ok(Event::Finished(
                    state
                        .finish_reason
                        .take()
                        .unwrap_or(FinishReason::Completed),
                )));
                return Ok(events);
            }

            let chunk: StreamChunk = match serde_json::from_str(&msg.data) {
                Ok(c) => c,
                Err(error) => {
                    warn!(
                        error = error.to_string(),
                        data = &msg.data,
                        "Failed to parse Cerebras chunk."
                    );
                    return Ok(vec![]);
                }
            };

            let mut events = Vec::new();

            for choice in &chunk.choices {
                let delta = &choice.delta;

                // Reasoning via `reasoning` (Cerebras parsed format) or
                // `reasoning_content` (DeepSeek-compatible). Both are
                // deserialized into `reasoning_content` via serde alias.
                if let Some(reasoning) = &delta.reasoning_content
                    && !reasoning.is_empty()
                {
                    events.push(Ok(Event::reasoning(0, reasoning.clone())));
                }

                // Content
                if let Some(content) = &delta.content
                    && !content.is_empty()
                {
                    if !state.reasoning_flushed {
                        events.push(Ok(Event::flush(0)));
                        state.reasoning_flushed = true;
                    }

                    if state.is_structured {
                        events.push(Ok(Event::structured(1, content.clone())));
                    } else {
                        events.push(Ok(Event::message(1, content.clone())));
                    }
                }

                // Tool calls
                if let Some(tool_calls) = &delta.tool_calls {
                    if !state.reasoning_flushed {
                        events.push(Ok(Event::flush(0)));
                        state.reasoning_flushed = true;
                    }

                    for tc in tool_calls {
                        let index = tc.index as usize + 2;

                        if !state.tool_call_indices.contains(&index) {
                            state.tool_call_indices.push(index);
                        }

                        let id = tc.id.clone().unwrap_or_default();
                        let name = tc
                            .function
                            .as_ref()
                            .and_then(|f| f.name.clone())
                            .unwrap_or_default();
                        if !id.is_empty() || !name.is_empty() {
                            events.push(Ok(Event::tool_call_start(index, id, name)));
                        }

                        if let Some(args) =
                            tc.function.as_ref().and_then(|f| f.arguments.as_deref())
                        {
                            events.push(Ok(Event::tool_call_args(index, args)));
                        }
                    }
                }

                // Finish reason
                if let Some(reason) = &choice.finish_reason {
                    if !state.reasoning_flushed {
                        events.push(Ok(Event::flush(0)));
                        state.reasoning_flushed = true;
                    }
                    events.push(Ok(Event::flush(1)));

                    if matches!(reason.as_str(), "tool_calls" | "stop") {
                        for &index in &state.tool_call_indices {
                            events.push(Ok(Event::flush(index)));
                        }
                        state.tool_call_indices.clear();
                    }

                    match reason.as_str() {
                        "length" => state.finish_reason = Some(FinishReason::MaxTokens),
                        "stop" => state.finish_reason = Some(FinishReason::Completed),
                        _ => {}
                    }
                }
            }

            Ok(events)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
#[path = "cerebras_tests.rs"]
mod tests;
