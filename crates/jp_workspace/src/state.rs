use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use jp_conversation::{Conversation, ConversationId, ConversationStream};
use parking_lot::RwLock;

/// The entire in-memory workspace state.
///
/// Each conversation's metadata and events are wrapped in `Arc<RwLock<...>>`
/// for shared ownership between the workspace and any active
/// `ConversationLock` / `ConversationMut` scopes.
///
/// The `OnceLock` provides lazy initialization — data is loaded from disk on
/// first access. The `Arc` enables shared ownership. The `RwLock` allows
/// concurrent reads and exclusive writes within the process.
#[derive(Debug, Default)]
pub(super) struct State {
    /// Conversation metadata for all conversations.
    pub(super) conversations: HashMap<ConversationId, OnceLock<Arc<RwLock<Conversation>>>>,

    /// Event streams for all conversations.
    pub(super) events: HashMap<ConversationId, OnceLock<Arc<RwLock<ConversationStream>>>>,
}
