//! Query draft management (`QUERY_MESSAGE.md`).
//!
//! Preserves the user's composed query in the conversation's user-local
//! directory until the turn starts, so an interrupted run can seed the next `jp
//! query --edit` buffer from the saved draft.

use std::{fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use jp_conversation::ConversationId;
use jp_storage::backend::FsStorageBackend;
use jp_workspace::ConversationLock;
use tracing::{debug, warn};

use crate::{ctx::Ctx, editor};

/// The store holding conversations' saved query drafts, or `None` when the run
/// writes nothing (`--no-persist`).
///
/// Every read for cleanup and every removal of a saved draft goes through this,
/// so a `--no-persist` run, which composes in a temporary directory, never
/// removes the draft a persisting run saved.
pub(super) fn draft_store(ctx: &Ctx) -> Option<&FsStorageBackend> {
    ctx.fs_backend
        .as_deref()
        .filter(|_| ctx.workspace.persists())
}

/// Resolve the directory where the query draft for conversation `id` should be
/// written, plus an optional temp directory guard when `persist` is `false`.
///
/// When persistence is enabled, the draft is placed in the conversation's
/// current user-local directory (using `stored_title` rather than any pending
/// title override so a subsequent title rename carries the draft along).
/// When `persist` is `false` (`--no-persist`), the draft is placed in a
/// temporary directory that is deleted when the returned guard drops, seeded
/// with a copy of the conversation's saved draft so the editor shows it.
///
/// # Errors
///
/// Fails when the temporary directory cannot be created, or the saved draft
/// cannot be copied into it.
pub(super) fn draft_dir(
    persist: bool,
    fs_backend: Option<&FsStorageBackend>,
    workspace_root: &Utf8Path,
    id: &ConversationId,
    stored_title: Option<&str>,
) -> io::Result<(Utf8PathBuf, Option<Utf8TempDir>)> {
    if !persist {
        let scratch = camino_tempfile::tempdir()?;
        let saved = fs_backend
            .and_then(|fs| fs.find_user_local_conversation_dir(id))
            .map(|dir| dir.join(editor::QUERY_FILENAME))
            .filter(|path| path.is_file());
        if let Some(saved) = saved {
            fs::copy(&saved, scratch.path().join(editor::QUERY_FILENAME))?;
        }
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

/// Policy for removing a stored query draft after a turn.
#[derive(Debug, Clone, Copy)]
pub(super) enum DraftRemoval<'a> {
    /// Remove the draft unconditionally.
    Any,

    /// Remove the draft only if its revision fingerprint still matches.
    ///
    /// If `None` (no draft existed when the turn started), any draft now on
    /// disk was written concurrently and is preserved.
    IfUnchanged(Option<&'a str>),
}

/// Return the revision fingerprint of conversation `id`'s user-local query
/// draft, or `None` if the draft does not exist or cannot be read.
pub(super) fn draft_fingerprint(
    fs_backend: Option<&FsStorageBackend>,
    id: &ConversationId,
) -> Option<String> {
    let dir = fs_backend.and_then(|fs| fs.find_user_local_conversation_dir(id))?;

    fs::read_to_string(dir.join(editor::QUERY_FILENAME))
        .ok()
        .map(|content| editor::draft_revision(&content))
}

/// Remove `QUERY_MESSAGE.md` from conversation `id`'s user-local directory
/// according to `removal`.
///
/// Re-resolves the live user-local directory by ID because the directory may
/// have been renamed mid-turn when a title was set.
/// Preserves the file if `removal` is [`DraftRemoval::IfUnchanged`] and the
/// file's fingerprint has changed since the request was sent.
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

/// Return `true` if the conversation has no saved query draft and its turn
/// count has not increased from `turns_before`.
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
