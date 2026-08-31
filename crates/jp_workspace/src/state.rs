use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, atomic::AtomicBool},
};

use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::backend::StoragePresence;
use parking_lot::RwLock;

/// The entire in-memory workspace state.
///
/// Each conversation's metadata and events are wrapped in `Arc<RwLock<...>>`
/// for shared ownership between the workspace and any active `ConversationLock`
/// / `ConversationMut` scopes.
///
/// The `OnceLock` provides lazy initialization — data is loaded from disk on
/// first access.
/// The `Arc` enables shared ownership.
/// The `RwLock` allows concurrent reads and exclusive writes within the
/// process.
#[derive(Debug, Default)]
pub(super) struct State {
    /// Conversation metadata for all conversations.
    pub(super) conversations: HashMap<ConversationId, OnceLock<Arc<RwLock<Conversation>>>>,

    /// Event streams for all conversations.
    pub(super) events: HashMap<ConversationId, OnceLock<Arc<RwLock<ConversationStream>>>>,

    /// Which storage roots hold each conversation.
    ///
    /// Populated from the cross-root index load and a conversation's creation
    /// intent.
    pub(super) presence: HashMap<ConversationId, StoragePresence>,

    /// Whether each conversation has ever been written to the store.
    ///
    /// Shared with any lock held on the conversation, which is what sets it: a
    /// write happens under a lock, and a lock has no route back here.
    ///
    /// The question it answers is "have we written this", not "is it on disk
    /// now" — an index scan already reports the latter.
    /// Only the former can tell a conversation created moments ago from one
    /// another process has since deleted, because both are absent from the
    /// scan.
    pub(super) written: HashMap<ConversationId, Arc<AtomicBool>>,
}
