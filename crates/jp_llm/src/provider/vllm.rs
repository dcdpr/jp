//! The vLLM provider: a self-hosted server that speaks the OpenAI-compatible
//! `/v1/chat/completions` dialect and checks a Bearer token.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use jp_attachment::AttachmentContent;
use jp_config::{
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ReasoningConfig,
    },
    providers::llm::vllm::VllmConfig,
};
use jp_conversation::thread::text_attachments_to_xml;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest_eventsource::{EventSource, retry::Never};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, trace, warn};

use super::{
    EventStream, ModelDetails,
    openai_compat::{assemble_event_stream, convert_events, convert_tool_choice, convert_tools},
    trace_to_tmpfile,
};
use crate::{error::Error, provider::Provider, query::ChatQuery, stream::with_tool_call_keepalive};

static PROVIDER: ProviderId = ProviderId::Vllm;

/// How often to inject a synthetic keep-alive while a tool call is streaming.
///
/// Stays below the enforced minimum `stream_idle_timeout_secs` (10s) so the
/// heartbeat always lands before the idle window elapses if the model pauses
/// between argument chunks.
const TOOL_CALL_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Vllm {
    client: reqwest::Client,
    base_url: String,
}

#[async_trait]
impl Provider for Vllm {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails, Error> {
        let id: ModelIdConfig = (PROVIDER, name.as_ref()).try_into()?;

        Ok(self
            .models()
            .await?
            .into_iter()
            .find(|m| m.id == id)
            .unwrap_or(ModelDetails::empty(id)))
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, Error> {
        self.client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<VllmModelList>()
            .await?
            .data
            .iter()
            .map(map_model)
            .collect::<Result<_, _>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream, Error> {
        debug!(model = %model.id.name, "Starting vLLM chat completion stream.");

        let (body, is_structured) = create_request(model, query)?;

        trace!(
            request = %trace_to_tmpfile("jp-vllm-request", &body),
            "Request payload."
        );

        let request = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("content-type", "application/json")
            .json(&body);

        let mut es =
            EventSource::new(request).map_err(|e| Error::InvalidResponse(e.to_string()))?;
        // Retries are owned by the stream retry layer; disable EventSource's
        // own auto-reconnect so a closed connection ends the stream instead of
        // silently re-issuing the request.
        es.set_retry_policy(Box::new(Never));

        Ok(with_tool_call_keepalive(
            assemble_event_stream(es, "vllm", is_structured),
            TOOL_CALL_KEEPALIVE_INTERVAL,
        ))
    }
}

#[cfg(test)]
impl Vllm {
    /// Build the vLLM wire request for `query` and serialize it to JSON without
    /// sending.
    /// Test-only seam for snapshotting request construction (notably compaction
    /// projection) across providers.
    #[expect(
        clippy::unused_self,
        reason = "uniform per-provider seam; only some providers read instance state"
    )]
    pub(crate) fn request_value(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<serde_json::Value, Error> {
        let (request, _) = create_request(model, query)?;
        Ok(request)
    }
}

/// Build the JSON request body for the vLLM `/v1/chat/completions` endpoint.
///
/// Returns `(body, is_structured)`.
fn create_request(model: &ModelDetails, query: ChatQuery) -> Result<(Value, bool), Error> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        ..
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

    // vLLM renders the request through the served model's own chat template,
    // and several of those templates reject a system message that isn't the
    // first message. Joining the parts keeps every served model reachable
    // regardless of its template.
    let mut messages: Vec<Value> = if system_parts.is_empty() {
        vec![]
    } else {
        vec![json!({ "role": "system", "content": system_parts.join("\n\n") })]
    };

    // Prepend binary image attachments as a user message with image_url
    // content blocks (OpenAI chat completions format).
    let image_blocks: Vec<_> = parts
        .attachments
        .iter()
        .filter_map(|a| match &a.content {
            AttachmentContent::Binary { data, media_type } if media_type.starts_with("image/") => {
                Some(json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!(
                            "data:{media_type};base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(data),
                        ),
                    },
                }))
            }
            AttachmentContent::Binary { media_type, .. } => {
                warn!(
                    source = %a.source,
                    media_type,
                    "Unsupported binary attachment media type for vLLM, skipping."
                );
                None
            }
            AttachmentContent::Text(_) => None,
        })
        .collect();

    if !image_blocks.is_empty() {
        messages.push(json!({
            "role": "user",
            "content": image_blocks,
        }));
    }

    messages.extend(convert_events(parts.events));
    let converted_tools = convert_tools(tools, &tool_choice);
    let tool_choice_val = convert_tool_choice(&tool_choice);

    trace!(
        slug,
        messages_size = messages.len(),
        tools_size = converted_tools.len(),
        "Built vLLM request."
    );

    // Models such as Qwen3 default to thinking-on, so
    // `chat_template_kwargs.enable_thinking` tells the chat template whether to
    // prompt the model to think at all. Models whose template doesn't read the
    // kwarg silently ignore it.
    let reasoning_enabled = !matches!(parameters.reasoning, None | Some(ReasoningConfig::Off));

    let mut body = json!({
        "model": slug,
        "messages": messages,
        "stream": true,
        "chat_template_kwargs": { "enable_thinking": reasoning_enabled },
    });

    if let Some(temperature) = parameters.temperature {
        body["temperature"] = json!(temperature);
    }

    if let Some(top_p) = parameters.top_p {
        body["top_p"] = json!(top_p);
    }

    // vLLM takes `top_k` as a sampling extension to the OpenAI body. Qwen3, the
    // family most often served this way, documents a `top_k` alongside its
    // `top_p`, so leaving it behind changes what the model was tuned for.
    if let Some(top_k) = parameters.top_k {
        body["top_k"] = json!(top_k);
    }

    if let Some(max_tokens) = parameters.max_tokens {
        body["max_tokens"] = json!(max_tokens);
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
                "schema": schema,
                "strict": true,
            },
        });
    }

    Ok((body, is_structured))
}

/// A `/v1/models` listing from vLLM.
///
/// vLLM serves the OpenAI shape and adds `max_model_len` per entry, which is
/// the context window the server was launched with.
#[derive(Debug, Deserialize)]
struct VllmModelList {
    #[serde(default)]
    data: Vec<VllmModel>,
}

#[derive(Debug, Deserialize)]
struct VllmModel {
    id: String,

    /// The served context window.
    ///
    /// Absent on servers that omit the vLLM extension fields.
    #[serde(default)]
    max_model_len: Option<u32>,
}

/// Map a vLLM model listing entry to model details.
///
/// The id keeps its full form, for example `Qwen/Qwen3-8B`, because vLLM
/// accepts only that form in a request.
fn map_model(model: &VllmModel) -> Result<ModelDetails, Error> {
    Ok(ModelDetails {
        id: (PROVIDER, model.id.as_str()).try_into()?,
        display_name: None,
        context_window: model.max_model_len,
        // vLLM reports no generation ceiling; it is bounded by the served
        // context rather than a per-model limit.
        max_output_tokens: None,
        // Reasoning is a server-launch concern for vLLM, selected with
        // `--reasoning-parser` rather than reported per model, so support stays
        // unknown and an explicit request is passed through.
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        prefill: None,
        subscription: None,
        features: vec![],
    })
}

impl TryFrom<&VllmConfig> for Vllm {
    type Error = Error;

    fn try_from(config: &VllmConfig) -> Result<Self, Self::Error> {
        let (api_key, _) =
            super::api_key_chain::resolve("vllm", &config.auth, &config.api_key_env)?;

        let client = reqwest::Client::builder()
            .default_headers(HeaderMap::from_iter([(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}"))
                    .map_err(|_| Error::InvalidResponse("invalid API key".into()))?,
            )]))
            .build()?;

        Ok(Vllm {
            client,
            base_url: config.base_url.clone(),
        })
    }
}

/// vLLM's recorded-test route.
#[cfg(test)]
pub(crate) static TEST_SUPPORT: super::ApiOnlyTestSupport = super::ApiOnlyTestSupport(&API_ROUTE);

#[cfg(test)]
static API_ROUTE: super::ApiTestRoute = super::ApiTestRoute {
    id: ProviderId::Vllm,
    base_url: |config| config.vllm.base_url.clone(),
    set_base_url: |config, url| config.vllm.base_url = url,
    use_replay_credentials: |config| {
        config.vllm.api_key_env = super::replay_credential_env().into();
    },
    model: || ModelDetails {
        id: "vllm/Qwen/Qwen3.8-Flash-Next-NVFP4".parse().unwrap(),
        display_name: None,
        context_window: Some(131_072),
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        prefill: None,
        subscription: None,
        features: vec![],
    },
};

#[cfg(test)]
#[path = "vllm_tests.rs"]
mod tests;
