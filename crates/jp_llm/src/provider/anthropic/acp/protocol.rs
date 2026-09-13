//! Typed Claude adapter notifications and response translation.

use std::collections::{HashMap, HashSet};

use agent_client_protocol::{
    JsonRpcNotification,
    schema::v1::{
        PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SelectedPermissionOutcome, SessionId, SessionNotification,
        SessionUpdate, ToolCallStatus,
    },
};
use async_anthropic::types::{CreateMessagesResponse, MessageContent, MessagesStreamEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::transcript::tool_name;
use crate::{
    error::StreamError,
    event::{Event, EventPart, FinishReason, ToolCallPart},
    provider::anthropic::map_event,
};

/// The adapter's effective authentication, independent of JP's token store.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "_auth/status_update")]
#[serde(rename_all = "camelCase")]
pub(super) struct AuthUpdate {
    pub auth_status: AgentAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum AgentAuth {
    Account {
        account: Account,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Account {
    pub plan: String,
}

impl AgentAuth {
    pub(super) fn is_subscription(&self) -> bool {
        matches!(self, Self::Account { account } if matches!(account.plan.as_str(), "Claude Pro" | "Claude Max" | "pro" | "max"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "_claude/sdkMessage")]
#[serde(rename_all = "camelCase")]
pub(super) struct SdkNotification {
    pub session_id: SessionId,
    pub message: SdkMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum SdkMessage {
    System {
        subtype: String,
        #[serde(default)]
        tools: Vec<String>,
    },
    StreamEvent {
        event: MessagesStreamEvent,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
    },
    User {
        message: UserMessage,
    },
    Assistant {
        message: CreateMessagesResponse,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
    },
    Result {
        subtype: String,
        is_error: bool,
        #[serde(default)]
        structured_output: Option<Value>,
        #[serde(default)]
        stop_reason: Option<String>,
        #[serde(default)]
        refusal: Option<Value>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct UserMessage {
    content: UserContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum UserContent {
    Blocks(Vec<UserBlock>),
    Text(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UserBlock {
    ToolResult {
        tool_use_id: String,
    },
    #[serde(other)]
    Other,
}

/// One request's translation state.
/// Replay never enters this state as live output.
pub(super) struct State {
    pub session: Option<SessionId>,
    pub live: bool,
    pub authenticated: bool,
    pub pending_tools: HashSet<String>,
    model: String,
    tools: HashMap<String, String>,
    structured: bool,
    inventory_checked: bool,
    seen_calls: HashSet<String>,
    observed_names: HashMap<String, String>,
    ignored: HashSet<usize>,
    index_base: usize,
    next_index: usize,
    pub final_events: Option<Vec<Event>>,
}

impl State {
    pub(super) fn new(
        model: String,
        tools: impl Iterator<Item = String>,
        structured: bool,
    ) -> Self {
        Self {
            session: None,
            live: false,
            authenticated: false,
            pending_tools: HashSet::new(),
            model,
            tools: tools.map(|name| (tool_name(&name), name)).collect(),
            structured,
            inventory_checked: false,
            seen_calls: HashSet::new(),
            observed_names: HashMap::new(),
            ignored: HashSet::new(),
            index_base: 0,
            next_index: 0,
            final_events: None,
        }
    }

    pub(super) fn permission(
        &mut self,
        request: RequestPermissionRequest,
    ) -> Result<(RequestPermissionResponse, Vec<Event>), StreamError> {
        if !self.live
            || !self.authenticated
            || !self.inventory_checked
            || self.session.as_ref() != Some(&request.session_id)
        {
            return Err(StreamError::other(
                "ACP requested tool execution outside an authenticated live request",
            ));
        }
        let call = request.tool_call;
        let id = call.tool_call_id.to_string();
        let native_name = call
            .meta
            .as_ref()
            .and_then(|meta| meta.get("claudeCode"))
            .and_then(|value| value.get("toolName"))
            .and_then(Value::as_str)
            .or_else(|| self.observed_names.get(&id).map(String::as_str))
            .ok_or_else(|| {
                StreamError::other("ACP permission request has no canonical tool name")
            })?;
        let name = self
            .tools
            .get(native_name)
            .ok_or_else(|| {
                StreamError::other(format!("ACP requested an unconfigured tool: {native_name}"))
            })?
            .clone();
        if !self.seen_calls.insert(id.clone()) {
            return Err(StreamError::other(
                "ACP repeated a dispatched tool-call identifier",
            ));
        }
        let arguments = call
            .fields
            .raw_input
            .and_then(|input| input.as_object().cloned())
            .ok_or_else(|| StreamError::other("ACP tool arguments must be a JSON object"))?;
        let option = request
            .options
            .into_iter()
            .find(|option| option.kind == PermissionOptionKind::AllowOnce)
            .ok_or_else(|| {
                StreamError::other("ACP did not offer one-call delegation to JP's MCP server")
            })?;
        self.pending_tools.insert(id.clone());
        let index = self.next_index;
        self.next_index += 1;
        let events = vec![
            Event::Part {
                index,
                part: EventPart::ToolCall(ToolCallPart::Start { id, name }),
                metadata: Map::new(),
            },
            Event::Part {
                index,
                part: EventPart::ToolCall(ToolCallPart::ArgumentChunk(
                    Value::Object(arguments).to_string(),
                )),
                metadata: Map::new(),
            },
            Event::flush(index),
            Event::Finished(FinishReason::Completed),
        ];
        Ok((
            RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(option.option_id),
            )),
            events,
        ))
    }

    pub(super) fn observe(&mut self, notification: SessionNotification) {
        if !self.live || self.session.as_ref() != Some(&notification.session_id) {
            return;
        }
        let (id, meta) = match notification.update {
            SessionUpdate::ToolCall(call) => (call.tool_call_id.to_string(), call.meta),
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.to_string();
                if matches!(
                    update.fields.status,
                    Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
                ) {
                    self.pending_tools.remove(&id);
                }
                (id, update.meta)
            }
            _ => return,
        };
        if let Some(name) = meta
            .as_ref()
            .and_then(|meta| meta.get("claudeCode"))
            .and_then(|value| value.get("toolName"))
            .and_then(Value::as_str)
        {
            self.observed_names.insert(id, name.to_owned());
        }
    }

    pub(super) fn sdk(&mut self, notification: SdkNotification) -> Result<Vec<Event>, StreamError> {
        if !self.live || self.session.as_ref() != Some(&notification.session_id) {
            return Ok(vec![]);
        }
        match notification.message {
            SdkMessage::System { subtype, tools } if subtype == "init" => {
                let expected: HashSet<_> = self.tools.keys().map(String::as_str).collect();
                let actual: HashSet<_> = tools
                    .iter()
                    .map(String::as_str)
                    .filter(|name| !(self.structured && *name == "StructuredOutput"))
                    .collect();
                if actual != expected {
                    return Err(StreamError::other(
                        "Claude Code's actual tool inventory differs from JP's configured tools",
                    ));
                }
                self.inventory_checked = true;
                Ok(vec![])
            }
            SdkMessage::User { message } => {
                if let UserContent::Blocks(blocks) = message.content {
                    for block in blocks {
                        if let UserBlock::ToolResult { tool_use_id } = block {
                            self.pending_tools.remove(&tool_use_id);
                        }
                    }
                }
                Ok(vec![])
            }
            SdkMessage::Assistant {
                message,
                parent_tool_use_id: None,
            } => {
                if message.model.as_deref() != Some(&self.model) {
                    return Err(StreamError::other(
                        "Claude Code answered with a different or unreported model",
                    ));
                }
                Ok(vec![])
            }
            SdkMessage::StreamEvent {
                event,
                parent_tool_use_id: None,
            } => self.stream_event(event),
            SdkMessage::Result {
                subtype,
                is_error,
                structured_output,
                stop_reason,
                refusal,
            } => {
                if !self.inventory_checked {
                    return Err(StreamError::other(
                        "Claude Code did not report its tool inventory",
                    ));
                }
                let mut events = vec![];
                let finish = if stop_reason.as_deref() == Some("refusal") {
                    FinishReason::Refused {
                        category: refusal
                            .as_ref()
                            .and_then(|r| r.get("category"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        explanation: refusal
                            .as_ref()
                            .and_then(|r| r.get("explanation"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    }
                } else if is_error || subtype != "success" {
                    return Err(StreamError::other(format!(
                        "Claude Code request failed: {subtype}"
                    )));
                } else {
                    if self.structured {
                        let data = structured_output.ok_or_else(|| {
                            StreamError::other("Claude Code returned no structured result")
                        })?;
                        events.push(Event::Part {
                            index: self.next_index,
                            part: EventPart::Structured(data.to_string()),
                            metadata: Map::new(),
                        });
                        events.push(Event::flush(self.next_index));
                        self.next_index += 1;
                    }
                    FinishReason::Completed
                };
                events.push(Event::Finished(finish));
                self.final_events = Some(events);
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }

    fn stream_event(&mut self, event: MessagesStreamEvent) -> Result<Vec<Event>, StreamError> {
        match &event {
            MessagesStreamEvent::MessageStart { .. } => {
                self.index_base = self.next_index;
            }
            MessagesStreamEvent::MessageStop => {
                self.index_base = self.next_index;
                self.ignored.clear();
                return Ok(vec![]);
            }
            MessagesStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                self.next_index = self.next_index.max(self.index_base + index + 1);
                if matches!(content_block, MessageContent::ToolUse(_))
                    || (self.structured && matches!(content_block, MessageContent::Text(_)))
                {
                    self.ignored.insert(*index);
                    return Ok(vec![]);
                }
            }
            MessagesStreamEvent::ContentBlockDelta { index, .. }
            | MessagesStreamEvent::ContentBlockStop { index }
                if self.ignored.contains(index) =>
            {
                return Ok(vec![]);
            }
            _ => {}
        }
        let mut events = Vec::new();
        for event in map_event(event, false) {
            let mut event = event?;
            match &mut event {
                Event::Part { index, .. } | Event::Flush { index, .. } => *index += self.index_base,
                Event::Finished(_) => continue,
                _ => {}
            }
            events.push(event);
        }
        Ok(events)
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
