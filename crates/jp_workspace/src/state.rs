//! Represents the in-memory state of the workspace.

use jp_conversation::{
    message::MessagePair, Context, ContextId, Conversation, ConversationId, ConversationsMetadata,
    Persona, PersonaId,
};
use jp_mcp::config::{McpServer, McpServerId};
use jp_tombmap::TombMap;
use serde::{Deserialize, Serialize};

/// Represents the entire in-memory state, both for the workspace and user-local
/// state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct State {
    pub local: LocalState,
    pub user: UserState,
}

/// Represents the entire in-memory workspace state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct LocalState {
    /// The active conversation.
    ///
    /// This is stored separately, to guarantee that an active conversation
    /// always exists.
    #[serde(skip)]
    pub active_conversation: Conversation,

    #[serde(skip_serializing_if = "TombMap::is_empty")]
    pub named_contexts: TombMap<ContextId, Context>,

    #[serde(skip_serializing_if = "TombMap::is_empty")]
    pub conversations: TombMap<ConversationId, Conversation>,

    #[serde(skip_serializing_if = "TombMap::is_empty")]
    pub messages: TombMap<ConversationId, Vec<MessagePair>>,

    #[serde(skip_serializing_if = "TombMap::is_empty")]
    pub personas: TombMap<PersonaId, Persona>,

    #[serde(skip_serializing_if = "TombMap::is_empty")]
    pub mcp_servers: TombMap<McpServerId, McpServer>,
}

/// Represents the entire in-memory local state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct UserState {
    pub conversations_metadata: ConversationsMetadata,
}
