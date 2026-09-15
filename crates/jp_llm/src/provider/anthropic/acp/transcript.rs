//! Claude-native records derived from the current JP Thread.
//!
//! The Anthropic request builder owns role grouping, projection, attachment
//! conversion, and reasoning compatibility.
//! This encoder preserves that content and adds disposable native bookkeeping
//! without rewriting tool-call IDs.

use async_anthropic::types::{
    Effort, ExtendedThinking, JsonOutputFormat, Message, MessageContent, MessageContentList,
    MessageRole, System, SystemContent,
};
use camino::Utf8Path;
use chrono::{DateTime, Utc};
use jp_config::{
    PartialAppConfig,
    model::{id::Name, parameters::ServiceTier},
};
use serde::Serialize;
use serde_json::{Map, Value};
use tracing::warn;
use uuid::Uuid;

use crate::{
    error::Result,
    model::ModelDetails,
    provider::anthropic::{BetaFeatures, CONTINUE_MESSAGE, create_request},
    query::ChatQuery,
};

/// Input for one isolated ACP session and its single pending prompt.
pub(super) struct PreparedRequest {
    pub system_prompt: String,
    pub history: Vec<Message>,
    pub prompt: String,
    pub model: Name,
    /// Explicit JP limit.
    /// Absence leaves the runtime's output limit unchanged.
    pub max_tokens: Option<u32>,
    pub thinking: Option<ExtendedThinking>,
    pub effort: Option<Effort>,
    pub schema: Option<Map<String, Value>>,
}

impl PreparedRequest {
    pub(super) fn new(model: &ModelDetails, mut query: ChatQuery) -> Result<Self> {
        let config = query.thread.events.config()?;
        let parameters = &config.assistant.model.parameters;
        let max_tokens = parameters.max_tokens;
        for (parameter, configured) in [
            ("temperature", parameters.temperature.is_some()),
            ("top_p", parameters.top_p.is_some()),
            ("top_k", parameters.top_k.is_some()),
            ("stop_words", !parameters.stop_words.is_empty()),
            (
                "service_tier",
                parameters
                    .service_tier
                    .is_some_and(|tier| tier != ServiceTier::Off),
            ),
        ] {
            if configured {
                warn!(
                    parameter,
                    "Ignoring unsupported model parameter for the ACP subscription flow"
                );
            }
        }
        for parameter in parameters.other.keys() {
            warn!(
                parameter,
                "Ignoring unsupported model parameter for the ACP subscription flow"
            );
        }
        // API-tier validation must not reject a request whose tier is omitted
        // from the SDK options. This delta affects only the owned request view.
        if parameters
            .service_tier
            .is_some_and(|tier| tier != ServiceTier::Off)
        {
            let mut delta = PartialAppConfig::default();
            delta.assistant.model.parameters.service_tier = Some(ServiceTier::Off);
            query.thread.events.add_config_delta(delta);
        }
        let beta = BetaFeatures(
            query
                .thread
                .events
                .config()?
                .providers
                .llm
                .anthropic
                .beta_headers
                .clone(),
        );
        let (mut request, _, _) = create_request(model, query, true, &beta, false)?;
        let system_prompt = match request.system.take() {
            Some(System::String(text)) => text,
            Some(System::Content(blocks)) => blocks
                .into_iter()
                .map(|block| {
                    let SystemContent::Text(text) = block;
                    text.text
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            None => String::new(),
        };
        let prompt = take_pending_text(&mut request.messages);
        for message in &mut request.messages {
            for block in &mut message.content.0 {
                match block {
                    MessageContent::Text(text) => text.cache_control = None,
                    MessageContent::ToolUse(call) => {
                        call.name = tool_name(&call.name);
                        call.cache_control = None;
                    }
                    MessageContent::ToolResult(result) => result.cache_control = None,
                    MessageContent::Document(document) => document.cache_control = None,
                    MessageContent::Thinking(_) | MessageContent::RedactedThinking { .. } => {}
                }
            }
        }
        let (effort, schema) = request.output_config.map_or((None, None), |output| {
            (
                output.effort,
                output.format.map(|format| {
                    let JsonOutputFormat::JsonSchema { schema } = format;
                    schema
                }),
            )
        });
        Ok(Self {
            system_prompt,
            history: request.messages,
            prompt,
            model: model.id.name.clone(),
            max_tokens,
            thinking: request.thinking,
            effort,
            schema,
        })
    }

    pub(super) fn records<'a>(
        &'a self,
        session: Uuid,
        cwd: &'a Utf8Path,
        timestamp: DateTime<Utc>,
    ) -> Vec<NativeRecord<'a>> {
        let mut parent = None;
        self.history
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let uuid = Uuid::new_v5(&session, &index.to_le_bytes());
                let record = NativeRecord {
                    type_: message.role.clone(),
                    uuid,
                    parent_uuid: parent,
                    session_id: session,
                    cwd,
                    is_sidechain: false,
                    timestamp,
                    message: match message.role {
                        MessageRole::User => NativeMessage::User {
                            content: &message.content,
                        },
                        MessageRole::Assistant => NativeMessage::Assistant {
                            content: &message.content,
                            id: format!("msg_jp_{}", uuid.simple()),
                            type_: MessageType::Message,
                            model: &self.model,
                            stop_reason: if message
                                .content
                                .0
                                .iter()
                                .any(|block| matches!(block, MessageContent::ToolUse(_)))
                            {
                                StopReason::ToolUse
                            } else {
                                StopReason::EndTurn
                            },
                            stop_sequence: None,
                            usage: NativeUsage {
                                input_tokens: 0,
                                output_tokens: 0,
                            },
                        },
                    },
                };
                parent = Some(uuid);
                record
            })
            .collect()
    }
}

/// MCP name assigned by Claude Code to a tool advertised by the `jp` server.
pub(super) fn tool_name(name: &str) -> String {
    format!("mcp__jp__{name}")
}

fn take_pending_text(messages: &mut Vec<Message>) -> String {
    let Some(last) = messages.last_mut() else {
        return CONTINUE_MESSAGE.to_owned();
    };
    if last.role != MessageRole::User
        || !matches!(last.content.0.last(), Some(MessageContent::Text(_)))
    {
        return CONTINUE_MESSAGE.to_owned();
    }
    let Some(MessageContent::Text(text)) = last.content.0.pop() else {
        unreachable!("the last block was checked above")
    };
    if last.content.0.is_empty() {
        messages.pop();
    }
    text.text
}

/// A record in the qualified Claude Code JSONL format.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NativeRecord<'a> {
    #[serde(rename = "type")]
    pub type_: MessageRole,
    pub uuid: Uuid,
    pub parent_uuid: Option<Uuid>,
    pub session_id: Uuid,
    pub cwd: &'a Utf8Path,
    pub is_sidechain: bool,
    pub timestamp: DateTime<Utc>,
    pub message: NativeMessage<'a>,
}

/// Model-visible content with bookkeeping restricted to assistant messages.
#[derive(Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub(super) enum NativeMessage<'a> {
    User {
        content: &'a MessageContentList,
    },
    Assistant {
        content: &'a MessageContentList,
        id: String,
        #[serde(rename = "type")]
        type_: MessageType,
        model: &'a Name,
        stop_reason: StopReason,
        stop_sequence: Option<String>,
        usage: NativeUsage,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MessageType {
    Message,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StopReason {
    EndTurn,
    ToolUse,
}

/// Native history bookkeeping, not a claim about previously billed usage.
#[derive(Serialize)]
pub(super) struct NativeUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
