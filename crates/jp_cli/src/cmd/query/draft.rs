//! The query draft: the request `jp query` composes, kept in the conversation's
//! user-local directory until a turn has sent it.
//!
//! The next `jp query --edit` seeds its buffer from the draft, so a request is
//! never lost to a run that ends before its turn starts.

use std::{fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use jp_conversation::ConversationId;
use jp_storage::backend::FsStorageBackend;
use jp_workspace::ConversationLock;
use tracing::{debug, warn};

use crate::editor;

/// Where the editor composes the query draft for conversation `id`, and the
/// temporary directory that holds it under `--no-persist`.
///
/// The draft lives in the conversation's user-local directory, and is never
/// projected into the committed workspace tree.
/// The directory is named for where the conversation lives now, not for the
/// title the run may end with: a write reconciles the ID to a single directory,
/// renaming the live one into the new name and deleting every other copy, so a
/// draft composed under the new name would be deleted while the rename carries
/// one in the live directory across.
///
/// Without `persist` nothing is written to storage, so the draft goes in a
/// temporary directory, removed when the returned guard drops.
pub(super) fn draft_dir(
    persist: bool,
    fs_backend: Option<&FsStorageBackend>,
    workspace_root: &Utf8Path,
    id: &ConversationId,
    stored_title: Option<&str>,
) -> io::Result<(Utf8PathBuf, Option<Utf8TempDir>)> {
    if !persist {
        let scratch = camino_tempfile::tempdir()?;
        return Ok((scratch.path().to_owned(), Some(scratch)));
    }

    let dir = fs_backend.map_or_else(
        || workspace_root.join(id.to_dirname(stored_title)),
        |fs| {
            fs.find_user_local_conversation_dir(id)
                .unwrap_or_else(|| fs.build_conversation_dir(id, stored_title, true))
        },
    );
    Ok((dir, None))
}

/// Write the pending request text to the conversation's user-local
/// query-message file, so an interrupt before the turn starts never discards
/// the composed request: the next `jp q --edit` session seeds its buffer from
/// this file.
///
/// Best-effort: failing to preserve a recovery copy must not fail the query
/// itself, so errors are logged and swallowed.
pub(super) fn preserve_query_message_file(conversation_dir: &Utf8Path, content: &str) {
    let path = conversation_dir.join(editor::QUERY_FILENAME);
    if let Err(error) =
        fs::create_dir_all(conversation_dir).and_then(|()| fs::write(&path, content))
    {
        warn!(path = %path, %error, "Failed to preserve query message file.");
    }
}

/// Which stored query drafts a cleanup may remove.
#[derive(Debug, Clone, Copy)]
pub(super) enum DraftRemoval<'a> {
    /// Remove whatever is stored.
    Any,

    /// Remove the draft only while it still matches this fingerprint.
    ///
    /// `None` means nothing was stored, so anything found now was written by
    /// someone else and is left alone.
    IfUnchanged(Option<&'a str>),
}

/// The fingerprint of a conversation's stored query draft, if it has one.
///
/// A draft that cannot be read has no fingerprint, which leaves it in place
/// rather than removing bytes nothing has seen.
pub(super) fn draft_fingerprint(
    fs_backend: Option<&FsStorageBackend>,
    id: &ConversationId,
) -> Option<String> {
    let dir = fs_backend.and_then(|fs| fs.find_user_local_conversation_dir(id))?;

    fs::read_to_string(dir.join(editor::QUERY_FILENAME))
        .ok()
        .map(|content| editor::draft_revision(&content))
}

/// Remove the editor's query-message file from a conversation's user-local
/// storage directory, tolerating its absence.
///
/// A title set mid-turn (e.g. from a leading markdown heading) renames the
/// directory, so the path captured before the turn is stale; re-resolve the
/// live directory instead.
/// The draft is written to user-local storage and never projected into the
/// workspace, so a single root is enough.
/// Without a filesystem backend or user-local store there is nothing to
/// resolve.
///
/// `removal` decides what may go: a draft replaced since the request was
/// composed belongs to whoever wrote it, and removing it would discard text
/// this invocation never saw.
pub(super) fn cleanup_query_message_file(
    fs_backend: Option<&FsStorageBackend>,
    id: &ConversationId,
    removal: DraftRemoval<'_>,
) {
    let Some(dir) = fs_backend.and_then(|fs| fs.find_user_local_conversation_dir(id)) else {
        return;
    };

    let path = dir.join(editor::QUERY_FILENAME);

    if let DraftRemoval::IfUnchanged(fingerprint) = removal {
        match fs::read_to_string(&path) {
            Ok(content) if Some(editor::draft_revision(&content).as_str()) != fingerprint => {
                debug!(path = %path, "Keeping a draft written since the request was composed.");
                return;
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => {
                warn!(path = %path, error = %e, "Failed to read query message file; keeping it.");
                return;
            }
        }
    }

    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => warn!(path = %path, error = %e, "Failed to remove query message file."),
    }
}

/// Whether the locked conversation holds no query draft, and no more turns than
/// the `turns_before` it started the run with.
pub(super) fn is_abandoned(
    fs_backend: Option<&FsStorageBackend>,
    lock: &ConversationLock,
    turns_before: usize,
) -> bool {
    let has_draft = fs_backend
        .and_then(|fs| fs.find_user_local_conversation_dir(&lock.id()))
        .is_some_and(|dir| dir.join(editor::QUERY_FILENAME).is_file());

    !has_draft && lock.with_events(|events| events.turn_count() == turns_before)
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
