//! Represents the in-memory state of the workspace.

use std::collections::HashMap;

use jp_conversation::{
    message::MessagePair, Context, ContextId, Conversation, ConversationId, Model, ModelId,
    Persona, PersonaId,
};
use jp_mcp::config::{McpServer, McpServerId};
use serde::{Deserialize, Serialize};

/// Represents the entire in-memory state, both for the workspace and user-local
/// state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct State {
    pub workspace: WorkspaceState,
    pub local: LocalState,
}

/// Represents the entire in-memory workspace state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceState {
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub named_contexts: HashMap<ContextId, Context>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub conversations: HashMap<ConversationId, Conversation>,

    /// The active conversation.
    ///
    /// This is stored separately, to guarantee that an active conversation
    /// always exists.
    #[serde(skip)]
    pub active_conversation: Conversation,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub messages: HashMap<ConversationId, Vec<MessagePair>>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub personas: HashMap<PersonaId, Persona>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub models: HashMap<ModelId, Model>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub mcp_servers: HashMap<McpServerId, McpServer>,
}

/// Represents the entire in-memory local state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct LocalState {
    pub conversations_metadata: ConversationsMetadata,
}

/// Holds metadata about all conversations, like the current active
/// conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ConversationsMetadata {
    /// The ID of the currently active conversation.
    ///
    /// If no active conversation exists, one is created.
    pub active_conversation_id: ConversationId,
}

impl ConversationsMetadata {
    pub fn new(active_conversation_id: ConversationId) -> Self {
        Self {
            active_conversation_id,
        }
    }
}

impl Default for ConversationsMetadata {
    fn default() -> Self {
        Self::new(ConversationId::default())
    }
}
