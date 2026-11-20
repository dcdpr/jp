//! A one-time migration of query drafts older JP versions left without their
//! conversation.
//!
//! Those versions wrote a new conversation's query draft before the
//! conversation itself, so a run that ended between the two left a directory
//! holding only the draft.
//! [`store_draft_only_conversations`] gives each such directory the files of a
//! conversation with no turns, so it is listed again and the draft can be
//! picked back up or the conversation removed.
//!
//! Temporary: once users no longer carry draft-only directories, this module
//! and its call in `run_inner` are deleted, and nothing else changes.

use std::sync::Arc;

use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationStream};
use jp_storage::{
    backend::{FsStorageBackend, LockBackend as _, PersistBackend as _, Projection},
    validate::{DirContents, UnstoredConversation, validate_conversation_dir},
};
use tracing::{debug, warn};

/// Store a conversation with no turns for every directory in `unstored` that
/// holds files.
///
/// The conversation is created when its ID says, with `config` as its base
/// config, and is written to the storage root its directory is in.
/// `config` is built only when there is something to store; `None` skips the
/// migration for this run.
///
/// A directory is left as it is while another process holds its conversation's
/// lock, and when its conversation also has a directory in the other storage
/// root, which the write would delete.
pub(crate) fn store_draft_only_conversations(
    fs: &FsStorageBackend,
    unstored: &[UnstoredConversation],
    config: impl FnOnce() -> Option<Arc<AppConfig>>,
) {
    let mut pending = unstored
        .iter()
        .filter(|entry| {
            entry
                .path
                .read_dir_utf8()
                .is_ok_and(|mut files| files.next().is_some())
        })
        .peekable();
    if pending.peek().is_none() {
        return;
    }

    let Some(config) = config() else {
        return;
    };
    let workspace = fs.conversations_dirs().into_iter().next();

    for entry in pending {
        let id = entry.id;
        let Ok(Some(_guard)) = fs.try_lock(&id.to_string(), None) else {
            debug!(%id, "Conversation is locked; storing its draft on a later run.");
            continue;
        };

        // Read again under the lock: another process may have stored it, or
        // given it a second directory, since the scan.
        let dirs = fs.live_conversation_dirs(&id);
        let unchanged = dirs == [entry.path.clone()]
            && matches!(
                validate_conversation_dir(&entry.path),
                Ok(DirContents::Unstored)
            );
        if !unchanged {
            debug!(%id, path = %entry.path, "Left draft-only directory: it changed.");
            continue;
        }

        let projection = if entry.path.parent() == workspace.as_deref() {
            Projection::Projected
        } else {
            Projection::LocalOnly
        };
        let metadata = Conversation::default().with_last_activated_at(id.timestamp());
        let events = ConversationStream::new(Arc::clone(&config)).with_created_at(id.timestamp());

        match fs.write(&id, &metadata, &events, projection) {
            Ok(()) => debug!(%id, path = %entry.path, "Stored a draft-only conversation."),
            Err(error) => warn!(%id, %error, "Failed to store a draft-only conversation."),
        }
    }
}

#[cfg(test)]
#[path = "draft_migration_tests.rs"]
mod tests;
