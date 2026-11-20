//! A one-time restore of the conversations older JP versions moved to the
//! trash.
//!
//! Those versions moved every conversation directory startup validation could
//! not load into `conversations/.trash/`, with a `TRASHED.md` beside it.
//! JP no longer has a trash: [`restore_trash`] moves every entry back into
//! `conversations/`, and the load-failure report and `jp doctor` report what is
//! still broken, the same as for any other directory there.
//!
//! Temporary: once users no longer carry a `.trash/`, this module and its call
//! in `run_inner` are deleted, and nothing else changes.

use std::{fs, io};

use camino::Utf8Path;
use jp_conversation::ConversationId;
use jp_storage::backend::{FsStorageBackend, LockBackend as _};
use tracing::{debug, warn};

const TRASH_DIR: &str = ".trash";
const TRASHED_FILE: &str = "TRASHED.md";

/// Move every entry of each storage root's `.trash/` back into that root's
/// `conversations/`, and remove the `.trash/` once it is empty.
///
/// An entry stays in the trash, and is tried again on the next run, while
/// another process holds its conversation's lock, or while a live directory for
/// the same conversation exists in any root: moving it back would give one
/// conversation two directories.
pub(crate) fn restore_trash(fs: &FsStorageBackend) {
    for conversations in fs.conversations_dirs() {
        restore_root(fs, &conversations);
    }
}

fn restore_root(fs: &FsStorageBackend, conversations: &Utf8Path) {
    let trash = conversations.join(TRASH_DIR);
    let Ok(entries) = trash.read_dir_utf8() else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        if entry.path().is_dir() {
            restore_entry(fs, conversations, entry.path(), entry.file_name());
        }
    }

    // Fails, and keeps the trash, while anything is left in it.
    if fs::remove_dir(&trash).is_ok() {
        debug!(%trash, "Removed the emptied trash.");
    }
}

fn restore_entry(
    fs: &FsStorageBackend,
    conversations: &Utf8Path,
    source: &Utf8Path,
    dirname: &str,
) {
    // Held across the check and the move, so no other process creates the
    // conversation in between. A name that is not a conversation ID has no
    // lock, and no process writes to it.
    let _guard = match ConversationId::try_from_dirname(dirname) {
        Ok(id) => {
            let Ok(Some(guard)) = fs.try_lock(&id.to_string(), None) else {
                debug!(%id, "Conversation is locked; restoring it on a later run.");
                return;
            };
            if let Some(live) = fs.live_conversation_dirs(&id).first() {
                warn!(%id, trash = %source, %live, "Left conversation in the trash: it is live.");
                return;
            }
            Some(guard)
        }
        Err(_) => None,
    };

    let target = conversations.join(dirname);
    if target.exists() {
        warn!(trash = %source, %target, "Left directory in the trash: its name is taken.");
        return;
    }
    if let Err(error) = fs::rename(source, &target) {
        warn!(trash = %source, %error, "Failed to restore directory from the trash.");
        return;
    }

    match fs::remove_file(target.join(TRASHED_FILE)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => warn!(path = %target, %error, "Failed to remove TRASHED.md."),
    }
    debug!(path = %target, "Restored directory from the trash.");
}

#[cfg(test)]
#[path = "trash_restore_tests.rs"]
mod tests;
