//! The ACP v1 wire types JP exchanges with the Claude Code adapter.
//!
//! This is the slice of the protocol JP actually reads or writes, not the whole
//! of it.
//! Serde ignores fields that are not declared, so a message carrying more than
//! this decodes fine and the extra fields are dropped; that is what keeps this
//! file proportional to JP's use rather than to the spec.
//!
//! Field names are the protocol's, spelled here rather than derived from the
//! Rust names: every struct carries `rename_all = "camelCase"` and `_meta` is
//! renamed by hand.
//! A mistake in one of them is a field that silently decodes as absent, which
//! is why `live_tests` records real adapter traffic and the ordinary suite
//! replays it.
//!
//! Optional fields are `#[serde(default)]` throughout.
//! The adapter omits anything it has no value for, and an absent field is never
//! an error.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Opaque protocol metadata, carried but not interpreted.
pub(super) type Meta = Map<String, Value>;

/// Declare one of the protocol's string-newtype identifiers.
///
/// Each is `serde(transparent)`, so it is a bare JSON string on the wire.
macro_rules! identifier {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        /// Public because [`super::Error`] names one; the module itself is
        /// private, so this does not widen the crate's surface.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

identifier!(
    /// Identifies one conversation with the adapter.
    SessionId
);
identifier!(
    /// Identifies one tool call within a session.
    ToolCallId
);
identifier!(
    /// Names a session setting, such as `model` or `mode`.
    SessionConfigId
);
identifier!(
    /// One selectable value of a session setting.
    SessionConfigValueId
);
identifier!(
    /// Identifies one choice offered in a permission request.
    PermissionOptionId
);

/// The protocol revision, a bare integer on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct ProtocolVersion(pub u16);

impl ProtocolVersion {
    /// The only revision JP speaks.
    pub(super) const V1: Self = Self(1);
}

/// Methods the agent handles, which JP calls.
pub(super) mod agent_method {
    pub(in super::super) const INITIALIZE: &str = "initialize";
    pub(in super::super) const SESSION_NEW: &str = "session/new";
    pub(in super::super) const SESSION_LOAD: &str = "session/load";
    pub(in super::super) const SESSION_SET_CONFIG_OPTION: &str = "session/set_config_option";
    pub(in super::super) const SESSION_PROMPT: &str = "session/prompt";
}

/// Methods the client handles, which the agent calls on JP.
pub(super) mod client_method {
    pub(in super::super) const SESSION_UPDATE: &str = "session/update";
    pub(in super::super) const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
}

// Initialization

/// Opens the connection and settles what each side supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InitializeRequest {
    pub protocol_version: ProtocolVersion,

    /// What JP can do on the agent's behalf.
    ///
    /// JP implements none of the optional client methods, so every capability
    /// here is false.
    /// It is sent rather than omitted so the agent never has to infer the
    /// answer from a missing field.
    pub client_capabilities: ClientCapabilities,
}

/// The optional client methods JP does not implement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClientCapabilities {
    pub fs: FileSystemCapabilities,

    /// Whether JP serves the `terminal/*` methods.
    pub terminal: bool,
}

/// The `fs/*` methods JP does not serve.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FileSystemCapabilities {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

/// What the agent settled on, and what it can do.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InitializeResponse {
    pub protocol_version: ProtocolVersion,

    #[serde(default)]
    pub agent_capabilities: AgentCapabilities,
}

/// The optional agent methods JP checks for before relying on them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentCapabilities {
    /// Whether `session/load` is available, which JP needs to supply history.
    #[serde(default)]
    pub load_session: bool,

    #[serde(default)]
    pub mcp_capabilities: McpCapabilities,
}

/// The MCP transports the agent can connect out over.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct McpCapabilities {
    /// Whether the agent can reach JP's loopback MCP endpoint.
    #[serde(default)]
    pub http: bool,
}

// Session setup

/// Starts a session with no prior history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NewSessionRequest {
    pub cwd: String,

    /// MCP servers the agent should connect to, as protocol objects.
    ///
    /// Left untyped because JP only ever sends one shape, the HTTP entry naming
    /// its own endpoint.
    pub mcp_servers: Vec<Value>,

    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// The session the agent created.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NewSessionResponse {
    pub session_id: SessionId,
}

/// Resumes a session whose transcript JP has already written to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LoadSessionRequest {
    pub session_id: SessionId,
    pub cwd: String,
    pub mcp_servers: Vec<Value>,

    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Acknowledges the load.
/// Everything it carries is optional and unread.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct LoadSessionResponse {}

// Session configuration

/// Selects a value for one session setting.
///
/// Only the id-valued form is sent: JP sets `model` and `mode`, both of which
/// the adapter models as selects.
/// A boolean setting would carry an extra `type` discriminator this does not
/// emit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetSessionConfigOptionRequest {
    pub session_id: SessionId,
    pub config_id: SessionConfigId,
    pub value: SessionConfigValueId,
}

/// Every setting and its value after the change.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetSessionConfigOptionResponse {
    #[serde(default)]
    pub config_options: Vec<SessionConfigOption>,
}

/// One setting, with the type-specific part flattened alongside it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionConfigOption {
    pub id: SessionConfigId,

    #[serde(flatten)]
    pub kind: SessionConfigKind,
}

/// A setting's shape and current value, discriminated by `type`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum SessionConfigKind {
    /// A choice from a list, which is how the adapter models `model` and
    /// `mode`.
    #[serde(rename_all = "camelCase")]
    Select { current_value: SessionConfigValueId },

    /// Any other shape.
    /// JP sets no such setting and reads none.
    #[serde(other)]
    Other,
}

// Prompting

/// Submits the user's turn and blocks until the agent finishes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PromptRequest {
    pub session_id: SessionId,

    /// The user's message as protocol content blocks.
    ///
    /// Left untyped because JP only ever sends one text block; everything else
    /// reaches the adapter through the transcript it loads.
    pub prompt: Vec<Value>,
}

/// Ends the turn.
/// Its stop reason is unread: JP takes the outcome from the SDK's own final
/// message instead, which carries the usage and refusal detail this does not.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct PromptResponse {}

// Session updates

/// One update about a session's progress.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionNotification {
    pub session_id: SessionId,
    pub update: SessionUpdate,
}

/// What the update is about, discriminated by `sessionUpdate`.
///
/// JP reads the two tool-call variants and ignores the rest: message and
/// thought chunks arrive again through the SDK's own notifications, which carry
/// the token usage JP needs.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub(super) enum SessionUpdate {
    /// A tool call has started.
    ToolCall(ToolCall),

    /// A tool call's status or output changed.
    ToolCallUpdate(ToolCallUpdate),

    /// Anything else the agent reports.
    #[serde(other)]
    Other,
}

/// A tool call the agent has begun.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ToolCall {
    pub tool_call_id: ToolCallId,

    #[serde(rename = "_meta", default)]
    pub meta: Option<Meta>,
}

/// A change to a tool call already in flight.
///
/// The protocol nests the mutable fields under a flattened object, so they
/// appear beside `toolCallId` on the wire and are declared flat here.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ToolCallUpdate {
    pub tool_call_id: ToolCallId,

    #[serde(default)]
    pub status: Option<ToolCallStatus>,

    /// The arguments the model produced, as the tool's own JSON object.
    #[serde(default)]
    pub raw_input: Option<Value>,

    #[serde(rename = "_meta", default)]
    pub meta: Option<Meta>,
}

/// How far along a tool call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,

    /// A status this build does not know.
    /// Treated as still running.
    #[serde(other)]
    Other,
}

// Permission

/// Asks JP to authorize one tool call before the agent runs it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestPermissionRequest {
    pub session_id: SessionId,

    /// The call awaiting authorization, in the same shape as an update.
    pub tool_call: ToolCallUpdate,

    pub options: Vec<PermissionOption>,
}

/// One answer the agent will accept.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PermissionOption {
    pub option_id: PermissionOptionId,
    pub kind: PermissionOptionKind,
}

/// What choosing an option means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PermissionOptionKind {
    /// Authorize this call only.
    /// The only kind JP selects: remembering a decision is the Host's job, not
    /// the adapter's.
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,

    /// A kind this build does not know.
    #[serde(other)]
    Other,
}

/// JP's answer to a permission request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestPermissionResponse {
    pub outcome: RequestPermissionOutcome,
}

/// The decision, discriminated by `outcome`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(super) enum RequestPermissionOutcome {
    /// JP chose one of the offered options.
    #[serde(rename_all = "camelCase")]
    Selected { option_id: PermissionOptionId },
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
