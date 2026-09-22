use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use jp_attachment::AttachmentContent;
use jp_config::{
    model::{
        id::{ModelIdConfig, Name, ProviderId},
        parameters::ReasoningConfig,
    },
    providers::llm::llamacpp::LlamacppConfig,
};
use jp_conversation::thread::text_attachments_to_xml;
use reqwest_eventsource::{EventSource, retry::Never};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, trace, warn};

use super::{
    EventStream, ModelDetails,
    openai_compat::{
        assemble_event_stream, convert_events, convert_tool_choice, convert_tools,
        to_system_messages,
    },
};
use crate::{error::Error, provider::Provider, query::ChatQuery, stream::with_tool_call_keepalive};

static PROVIDER: ProviderId = ProviderId::Llamacpp;

/// How often to inject a synthetic keep-alive while a tool call is streaming.
///
/// Stays below the enforced minimum `stream_idle_timeout_secs` (10s) so the
/// heartbeat always lands before the idle window elapses if the model pauses
/// between argument chunks.
const TOOL_CALL_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Llamacpp {
    reqwest_client: reqwest::Client,
    base_url: String,
}

#[async_trait]
impl Provider for Llamacpp {
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
        // The served context window, which is what a request is actually bounded
        // by. Best effort: older builds expose no `/props`, and the trained
        // length is a usable second choice.
        let served_context = self.served_context().await;

        self.reqwest_client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<LlamacppModelList>()
            .await?
            .data
            .iter()
            .map(|model| map_model(model, served_context))
            .collect::<Result<_, _>>()
    }

    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream, Error> {
        debug!(
            model = %model.id.name,
            "Starting Llamacpp chat completion stream."
        );

        let (body, is_structured) = create_request(model, query)?;

        trace!(
            body = serde_json::to_string(&body).unwrap_or_default(),
            "Sending request to Llamacpp."
        );

        let request = self
            .reqwest_client
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
            assemble_event_stream(es, "llamacpp", is_structured),
            TOOL_CALL_KEEPALIVE_INTERVAL,
        ))
    }
}

#[cfg(test)]
impl Llamacpp {
    /// Build the llama.cpp wire request for `query` and serialize it to JSON
    /// without sending.
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

/// Build the JSON request body for the llama.cpp `/v1/chat/completions`
/// endpoint.
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

    let mut messages: Vec<Value> = to_system_messages(system_parts).collect();

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
                    "Unsupported binary attachment media type for llama.cpp, skipping."
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
        "Built Llamacpp request."
    );

    // Like Ollama, llama.cpp models may default to thinking-on, so
    // `chat_template_kwargs.enable_thinking` tells the chat template whether to
    // prompt the model to think at all. Models whose template doesn't read the
    // kwarg silently ignore it.
    let reasoning_enabled = !matches!(parameters.reasoning, None | Some(ReasoningConfig::Off));

    // `reasoning_format` is a separate concern: it says how the server should
    // parse a thinking block, not whether one is produced. `deepseek` lifts it
    // into its own field, which is also what lets a grammar apply to the answer
    // that follows. Asking for `none` instead leaves the server no place to put
    // the grammar, and a structured-output request is refused outright with
    // "Failed to initialize samplers" for any thinking-capable model. Parsing
    // unconditionally costs nothing when the model does not think, and keeps
    // stray reasoning out of the answer when a template ignores the kwarg.
    let mut body = json!({
        "model": slug,
        "messages": messages,
        "stream": true,
        "reasoning_format": "deepseek",
        "chat_template_kwargs": { "enable_thinking": reasoning_enabled },
    });

    if let Some(temperature) = parameters.temperature {
        body["temperature"] = json!(temperature);
    }

    if let Some(top_p) = parameters.top_p {
        body["top_p"] = json!(top_p);
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

impl Llamacpp {
    /// The context size the server was launched with, from `/props`.
    ///
    /// Best effort by design: `/props` is secondary to the model listing, so a
    /// failure falls back to the trained length rather than failing the
    /// listing.
    async fn served_context(&self) -> Option<u32> {
        let url = format!("{}/props", self.base_url);

        let result = async {
            self.reqwest_client
                .get(&url)
                .send()
                .await?
                .error_for_status()?
                .json::<LlamacppProps>()
                .await
        }
        .await;

        match result {
            Ok(props) => props.default_generation_settings.n_ctx,
            Err(error) => {
                debug!(
                    %url,
                    %error,
                    "llama.cpp `/props` unavailable; falling back to the trained context length."
                );
                None
            }
        }
    }
}

/// The context size llama.cpp was launched with, from `/props`.
///
/// `None` when the endpoint is unavailable or reports no size, which includes
/// older builds predating `/props`.
#[derive(Debug, Deserialize)]
struct LlamacppProps {
    #[serde(default)]
    default_generation_settings: LlamacppGenerationSettings,
}

#[derive(Debug, Default, Deserialize)]
struct LlamacppGenerationSettings {
    #[serde(default)]
    n_ctx: Option<u32>,
}

/// A `/v1/models` listing from llama.cpp.
///
/// llama.cpp serves the OpenAI shape but adds a `meta` object per entry, so
/// this is modelled separately rather than widening the OpenAI type with fields
/// only one provider sends.
#[derive(Debug, Deserialize)]
struct LlamacppModelList {
    #[serde(default)]
    data: Vec<LlamacppModel>,
}

#[derive(Debug, Deserialize)]
struct LlamacppModel {
    id: String,

    #[serde(default)]
    meta: LlamacppMeta,
}

/// Loaded-model metadata reported by llama.cpp.
#[derive(Debug, Default, Deserialize)]
struct LlamacppMeta {
    /// Context length the model was trained for.
    ///
    /// Absent on older llama.cpp builds.
    #[serde(default)]
    n_ctx_train: Option<u32>,
}

/// Map a llama.cpp model, preferring the served context window over the trained
/// one.
///
/// `served_context` comes from `/props` and is what a request is actually
/// bounded by; `meta.n_ctx_train` is what the model was trained for and is
/// commonly far larger than the size the server was launched with.
/// Falling back to it can over-report, which is why the served value wins when
/// available.
fn map_model(model: &LlamacppModel, served_context: Option<u32>) -> Result<ModelDetails, Error> {
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
        context_window: served_context.or(model.meta.n_ctx_train),
        // llama.cpp reports no generation ceiling; it is bounded by the served
        // context rather than a per-model limit.
        max_output_tokens: None,
        // Reasoning is a server-launch concern for llama.cpp, selected with
        // `--reasoning-format` rather than reported per model, so support stays
        // unknown and an explicit request is passed through.
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        prefill: None,
        features: vec![],
    })
}

impl TryFrom<&LlamacppConfig> for Llamacpp {
    type Error = Error;

    fn try_from(config: &LlamacppConfig) -> Result<Self, Self::Error> {
        let reqwest_client = reqwest::Client::builder().build()?;
        let base_url = config.base_url.clone();

        Ok(Llamacpp {
            reqwest_client,
            base_url,
        })
    }
}

#[cfg(test)]
#[path = "llamacpp_tests.rs"]
mod tests;
