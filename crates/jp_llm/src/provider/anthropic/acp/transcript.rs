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
use serde_json::{Value, json};
use tracing::warn;
use uuid::Uuid;

use super::context::SessionContext;
use crate::{
    error::Result,
    model::ModelDetails,
    provider::{
        CONTINUE_MESSAGE,
        anthropic::{BetaFeatures, create_request},
    },
    query::ChatQuery,
};

/// Input for one isolated ACP session and its single pending prompt.
pub(super) struct PreparedRequest {
    pub system_prompt: String,
    pub history: Vec<Message>,
    /// Text blocks of the `session/prompt` request, in order.
    pub prompt: Vec<String>,
    /// When the conversation's first request was made, if it has one.
    pub started: Option<DateTime<Utc>>,
    pub model: Name,
    /// Explicit JP limit.
    /// Absence leaves the runtime's output limit unchanged.
    pub max_tokens: Option<u32>,
    pub thinking: Option<ExtendedThinking>,
    pub effort: Option<Effort>,
    pub schema: Option<JsonOutputFormat>,
}

impl PreparedRequest {
    pub(super) fn new(model: &ModelDetails, mut query: ChatQuery) -> Result<Self> {
        let started = query
            .thread
            .events
            .iter()
            .find_map(|event| event.is_chat_request().then_some(event.timestamp));
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
        let pending = split_trailing_text(&mut request.messages);
        let prompt = if pending.is_empty() {
            vec![CONTINUE_MESSAGE.to_owned()]
        } else {
            pending
        };
        let (effort, schema) = request
            .output_config
            .map_or((None, None), |output| (output.effort, output.format));
        Ok(Self {
            system_prompt,
            history: request.messages,
            prompt,
            started,
            model: model.id.name.clone(),
            max_tokens,
            thinking: request.thinking,
            effort,
            schema,
        })
    }

    /// The transcript records for `history`, shaped as Claude Code shaped the
    /// requests that sent it.
    ///
    /// Claude Code received each run of user messages ending in text as a
    /// prompt.
    /// When the run held more than text (attachments on the first turn, tool
    /// results before a new turn), the transcript it resumed ended in a user
    /// message, so it inserted an assistant message reading "No response
    /// requested." before the prompt (anthropics/claude-code#109781).
    /// Such a run is written the same way: its other blocks, that reply, then
    /// the prompt as a message of its own.
    /// With a `context`, the first prompt is followed by the context records
    /// Claude Code adds after a session's first prompt (see
    /// [`super::context`]).
    ///
    /// Claude Code then builds the request it built when it sent each prompt,
    /// so the history is read from the prompt cache.
    pub(super) fn records(
        &self,
        session: Uuid,
        cwd: &Utf8Path,
        timestamp: DateTime<Utc>,
        context: Option<&SessionContext>,
    ) -> JsonResult<Vec<Value>> {
        let mut writer = RecordWriter {
            session,
            cwd,
            timestamp,
            model: &self.model,
            records: vec![],
            parent: None,
            count: 0,
        };
        let mut first_prompt = true;
        let mut rest = self.history.as_slice();
        while let Some(first) = rest.first() {
            if first.role != MessageRole::User {
                writer.native(first)?;
                rest = &rest[1..];
                continue;
            }
            let count = rest
                .iter()
                .take_while(|message| message.role == MessageRole::User)
                .count();
            let (run, tail) = rest.split_at(count);
            rest = tail;

            let mut remainder = run.to_vec();
            let prompt = split_trailing_text(&mut remainder);
            for message in &remainder {
                writer.native(message)?;
            }
            if prompt.is_empty() {
                continue;
            }
            if !remainder.is_empty() {
                writer.synthetic_reply();
            }
            writer.native(&Message {
                role: MessageRole::User,
                content: MessageContentList(
                    prompt
                        .iter()
                        .map(|text| MessageContent::Text(text.as_str().into()))
                        .collect(),
                ),
            })?;
            if first_prompt && let Some(context) = context {
                for attachment in context.attachments() {
                    writer.attachment(&attachment);
                }
            }
            first_prompt = false;
        }
        Ok(writer.records)
    }
}

/// The text of the assistant message Claude Code inserts after a resumed
/// transcript that ends in a user message.
const SYNTHETIC_REPLY: &str = "No response requested.";

/// The outcome of serializing transcript content.
type JsonResult<T> = std::result::Result<T, serde_json::Error>;

/// Builds transcript records in chain order, each the parent of the next.
struct RecordWriter<'a> {
    session: Uuid,
    cwd: &'a Utf8Path,
    timestamp: DateTime<Utc>,
    model: &'a Name,
    records: Vec<Value>,
    parent: Option<Uuid>,
    /// How many records were written, which seeds their UUIDs.
    count: u64,
}

impl RecordWriter<'_> {
    fn next_uuid(&mut self) -> Uuid {
        let uuid = Uuid::new_v5(&self.session, &self.count.to_le_bytes());
        self.count += 1;
        uuid
    }

    fn push(&mut self, uuid: Uuid, record: Value) {
        self.records.push(record);
        self.parent = Some(uuid);
    }

    fn native(&mut self, message: &Message) -> JsonResult<()> {
        let uuid = self.next_uuid();
        let record = NativeRecord {
            type_: message.role.clone(),
            uuid,
            parent_uuid: self.parent,
            session_id: self.session,
            cwd: self.cwd,
            is_sidechain: false,
            timestamp: self.timestamp,
            message: match message.role {
                MessageRole::User => NativeMessage::User {
                    content: &message.content,
                },
                MessageRole::Assistant => NativeMessage::Assistant {
                    content: &message.content,
                    id: format!("msg_jp_{}", uuid.simple()),
                    type_: MessageType::Message,
                    model: self.model,
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
        let record = serde_json::to_value(record)?;
        self.push(uuid, record);
        Ok(())
    }

    /// The assistant message Claude Code inserts, as Claude Code records it.
    fn synthetic_reply(&mut self) {
        let uuid = self.next_uuid();
        let record = json!({
            "type": "assistant",
            "uuid": uuid,
            "parentUuid": self.parent,
            "sessionId": self.session,
            "cwd": self.cwd,
            "isSidechain": false,
            "isApiErrorMessage": false,
            "timestamp": self.timestamp,
            "message": {
                "id": format!("msg_jp_{}", uuid.simple()),
                "type": "message",
                "role": "assistant",
                "model": "<synthetic>",
                "content": [{"type": "text", "text": SYNTHETIC_REPLY}],
                "stop_reason": "stop_sequence",
                "stop_sequence": "",
                "usage": {"input_tokens": 0, "output_tokens": 0},
            },
        });
        self.push(uuid, record);
    }

    fn attachment(&mut self, attachment: &Value) {
        let uuid = self.next_uuid();
        let record = json!({
            "type": "attachment",
            "uuid": uuid,
            "parentUuid": self.parent,
            "sessionId": self.session,
            "cwd": self.cwd,
            "isSidechain": false,
            "timestamp": self.timestamp,
            "attachment": attachment,
        });
        self.push(uuid, record);
    }
}

/// MCP name assigned by Claude Code to a tool advertised by the `jp` server.
pub(super) fn tool_name(name: &str) -> String {
    format!("mcp__jp__{name}")
}

/// Remove the trailing user text blocks from `messages` and return them, for
/// the `session/prompt` request.
///
/// Pops every trailing text block across consecutive user messages, so the
/// transcript ends on an assistant message whenever the pending user messages
/// contain only text.
/// If a transcript ends in a user message, Claude Code inserts an assistant
/// message reading `"No response requested."` after it on load
/// (anthropics/claude-code#109781).
///
/// Tool results and document attachments stay in `messages`, so a transcript
/// ending in either still gets that inserted message.
/// Tool results have no `session/prompt` representation, and attachments stay
/// in the transcript as native `document` blocks with their title and context
/// in the cached prefix.
///
/// Returns an empty list when no trailing user text block is present.
fn split_trailing_text(messages: &mut Vec<Message>) -> Vec<String> {
    let mut prompt = Vec::new();
    while let Some(last) = messages.last_mut()
        && last.role == MessageRole::User
    {
        while let Some(block) = last.content.0.pop() {
            let MessageContent::Text(text) = block else {
                last.content.0.push(block);
                break;
            };
            prompt.push(text.text);
        }
        if !last.content.0.is_empty() {
            break;
        }
        messages.pop();
    }
    prompt.reverse();
    prompt
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

/// Placeholder title written to every Claude Code session JP opens.
///
/// If a session has no title, the adapter generates one with a background Haiku
/// request billed to the subscription.
/// JP manages conversation titles itself and never reads the Claude Code
/// session title.
pub(super) const SESSION_TITLE: &str = "JP";

/// A `custom-title` record in the Claude Code JSONL transcript format.
///
/// This record is session metadata only; it carries no message and is not sent
/// to the model.
#[derive(Serialize)]
#[serde(tag = "type", rename = "custom-title", rename_all = "camelCase")]
pub(super) struct TitleRecord {
    pub custom_title: &'static str,
    pub session_id: Uuid,
}

impl TitleRecord {
    pub(super) fn new(session: Uuid) -> Self {
        Self {
            custom_title: SESSION_TITLE,
            session_id: session,
        }
    }
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
