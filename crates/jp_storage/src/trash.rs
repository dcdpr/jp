//! The conversation trash.
//!
//! Each `conversations/` directory has a `.trash/` beside its live
//! conversations, holding the conversation directories a user moved there with
//! [`Storage::trash_conversation`].
//! The trash holds at most one entry per conversation in each storage root, so
//! [`Storage::untrash_conversation`] always knows which directory to move back.
//! Nothing else reads the trash: a trashed conversation is not listed, loaded,
//! or reported.

use std::{fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;
use tracing::warn;

use crate::{
    CONVERSATIONS_DIR, Storage, conversation_dirs_for_id, error::Error,
    resource_lock::ResourceGuard,
};

/// The trash directory inside each `conversations/` directory.
pub const TRASH_DIR: &str = ".trash";

/// The note written into every trashed conversation directory.
pub const TRASHED_FILE: &str = "TRASHED.md";

/// A conversation directory moved into or out of the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub from: Utf8PathBuf,
    pub to: Utf8PathBuf,
}

/// Why a conversation could not be moved into or out of the trash.
#[derive(Debug, thiserror::Error)]
pub enum TrashError {
    /// No live directory for the conversation exists in any storage root.
    #[error("no conversation with this ID")]
    NotFound,

    /// No trash entry for the conversation exists in any storage root.
    #[error("not in the trash")]
    NotTrashed,

    /// Another process holds the conversation's lock.
    #[error("in use by another process")]
    Locked,

    /// The trash already holds an entry for the conversation.
    #[error("the trash already holds it at {0}")]
    AlreadyTrashed(Utf8PathBuf),

    /// A live directory for the conversation exists.
    #[error("a conversation with this ID exists at {0}")]
    Exists(Utf8PathBuf),

    #[error(transparent)]
    Storage(#[from] Error),
}

impl Storage {
    /// Move every live directory of conversation `id`, in every storage root,
    /// into that root's trash, with a `TRASHED.md` saying how to move it back.
    ///
    /// Works on any directory named for the conversation, whether or not it
    /// loads.
    /// Holds the conversation's lock while it moves.
    ///
    /// # Errors
    ///
    /// Returns [`TrashError::AlreadyTrashed`] when a trash already holds the
    /// conversation, [`TrashError::NotFound`] when there is nothing to move,
    /// and [`TrashError::Locked`] when another process holds the conversation.
    /// Nothing is moved in those cases.
    pub fn trash_conversation(
        &self,
        id: &ConversationId,
        now: DateTime<Utc>,
    ) -> Result<Vec<Moved>, TrashError> {
        let _guard = self.lock_for_move(id)?;
        let moves = self.trash_moves(id)?;

        let note = trashed_note(id, now);
        for moved in &moves {
            let trash = moved.to.parent().unwrap_or(&moved.to);
            fs::create_dir_all(trash).map_err(|error| Error::write_failed(trash, error))?;
            rename(moved)?;

            let path = moved.to.join(TRASHED_FILE);
            fs::write(&path, &note).map_err(|error| Error::write_failed(path, error))?;
        }

        Ok(moves)
    }

    /// Move every trash entry of conversation `id`, in every storage root, back
    /// among that root's live conversations, and remove its `TRASHED.md`.
    ///
    /// Holds the conversation's lock while it moves.
    ///
    /// # Errors
    ///
    /// Returns [`TrashError::NotTrashed`] when no trash holds the conversation,
    /// [`TrashError::Exists`] when a live directory for it exists, and
    /// [`TrashError::Locked`] when another process holds the conversation.
    /// Nothing is moved in those cases.
    pub fn untrash_conversation(&self, id: &ConversationId) -> Result<Vec<Moved>, TrashError> {
        let _guard = self.lock_for_move(id)?;
        let moves = self.untrash_moves(id)?;

        for moved in &moves {
            rename(moved)?;

            let note = moved.to.join(TRASHED_FILE);
            match fs::remove_file(&note) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => warn!(path = %note, %error, "Failed to remove TRASHED.md."),
            }
        }

        Ok(moves)
    }

    /// The moves [`Self::trash_conversation`] would make, without making them.
    ///
    /// Writes nothing, not even a lock file.
    ///
    /// # Errors
    ///
    /// Returns the error [`Self::trash_conversation`] would return.
    pub fn plan_trash(&self, id: &ConversationId) -> Result<Vec<Moved>, TrashError> {
        self.ensure_unlocked(id)?;
        self.trash_moves(id)
    }

    /// The moves [`Self::untrash_conversation`] would make, without making
    /// them.
    ///
    /// Writes nothing, not even a lock file.
    ///
    /// # Errors
    ///
    /// Returns the error [`Self::untrash_conversation`] would return.
    pub fn plan_untrash(&self, id: &ConversationId) -> Result<Vec<Moved>, TrashError> {
        self.ensure_unlocked(id)?;
        self.untrash_moves(id)
    }

    /// Every live directory of `id` paired with its place in the trash, after
    /// the checks that would refuse the move.
    fn trash_moves(&self, id: &ConversationId) -> Result<Vec<Moved>, TrashError> {
        let prefix = id.to_dirname(None);
        let roots = self.conversations_dirs();

        if let Some(path) = dirs_for_id(&roots, TRASH_DIR, &prefix).next() {
            return Err(TrashError::AlreadyTrashed(path));
        }

        let moves: Vec<_> = roots
            .iter()
            .flat_map(|conversations| {
                conversation_dirs_for_id(conversations, &prefix)
                    .into_iter()
                    .map(|from| Moved {
                        to: conversations.join(TRASH_DIR).join(dirname(&from)),
                        from,
                    })
            })
            .collect();
        if moves.is_empty() {
            return Err(TrashError::NotFound);
        }

        Ok(moves)
    }

    /// Every trash entry of `id` paired with its live place, after the checks
    /// that would refuse the move.
    fn untrash_moves(&self, id: &ConversationId) -> Result<Vec<Moved>, TrashError> {
        let prefix = id.to_dirname(None);
        let roots = self.conversations_dirs();

        let moves: Vec<_> = roots
            .iter()
            .flat_map(|conversations| {
                conversation_dirs_for_id(&conversations.join(TRASH_DIR), &prefix)
                    .into_iter()
                    .map(|from| Moved {
                        to: conversations.join(dirname(&from)),
                        from,
                    })
            })
            .collect();
        if moves.is_empty() {
            return Err(TrashError::NotTrashed);
        }

        if let Some(path) = dirs_for_id(&roots, "", &prefix).next() {
            return Err(TrashError::Exists(path));
        }

        Ok(moves)
    }

    fn ensure_unlocked(&self, id: &ConversationId) -> Result<(), TrashError> {
        if self.is_conversation_locked(&id.to_string()) {
            return Err(TrashError::Locked);
        }
        Ok(())
    }

    /// The live `conversations/` directory of each storage root, workspace
    /// first.
    pub(crate) fn conversations_dirs(&self) -> Vec<Utf8PathBuf> {
        [Some(&self.root), self.user.as_ref()]
            .into_iter()
            .flatten()
            .map(|root| root.join(CONVERSATIONS_DIR))
            .collect()
    }

    fn lock_for_move(&self, id: &ConversationId) -> Result<Box<dyn ResourceGuard>, TrashError> {
        self.try_lock_conversation(&id.to_string(), None)?
            .ok_or(TrashError::Locked)
    }
}

/// Directories for the conversation `prefix` names under `subdir` of each
/// conversations directory in `roots`.
fn dirs_for_id<'a>(
    roots: &'a [Utf8PathBuf],
    subdir: &'a str,
    prefix: &'a str,
) -> impl Iterator<Item = Utf8PathBuf> + 'a {
    roots.iter().flat_map(move |conversations| {
        conversation_dirs_for_id(&conversations.join(subdir), prefix)
    })
}

fn dirname(dir: &Utf8Path) -> &str {
    dir.file_name().unwrap_or_default()
}

fn rename(moved: &Moved) -> Result<(), Error> {
    fs::rename(&moved.from, &moved.to)
        .map_err(|error| Error::rename_failed(&moved.from, &moved.to, error))
}

fn trashed_note(id: &ConversationId, now: DateTime<Utc>) -> String {
    format!(
        "# Trashed Conversation\n\nMoved here by `jp conversation trash` on {}.\n\nRun `jp \
         conversation untrash {id}` to move it back.\n",
        now.format("%Y-%m-%dT%H:%M:%SZ")
    )
}

#[cfg(test)]
#[path = "trash_tests.rs"]
mod tests;
