use std::cell::OnceCell;

use jp_conversation::{Conversation, ConversationId, ConversationStream, ConversationsMetadata};
use jp_tombmap::TombMap;
use serde::{Deserialize, Serialize};

/// Represents the entire in-memory state, both for the workspace and user-local
/// state.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub local: LocalState,
    pub user: UserState,
}

/// Represents the entire in-memory workspace state.
#[derive(Debug, Default)]
pub(crate) struct LocalState {
    /// The active conversation.
    ///
    /// This is stored separately, to guarantee that an active conversation
    /// always exists.
    pub active_conversation: Conversation,

    /// The mapping of conversation IDs to conversation metadata.
    ///
    /// The metadata is stored as a `OnceCell` to allow for lazy initialization
    /// of the conversation metadata, which can be expensive to load.
    pub conversations: TombMap<ConversationId, OnceCell<Conversation>>,

    /// The mapping of conversation IDs to conversation events.
    ///
    /// The events are stored as a `OnceCell` to allow for lazy initialization
    /// of the conversation events, which can be expensive to load.
    pub events: TombMap<ConversationId, OnceCell<ConversationStream>>,
}

/// Represents the entire in-memory local state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct UserState {
    pub conversations_metadata: ConversationsMetadata,
}
