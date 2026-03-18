use jp_conversation::{ConversationId, ConversationsMetadata};
use jp_storage::{
    Storage,
    validate::{InvalidConversation, ValidationError},
};
use tracing::error;

use crate::{
    Workspace,
    error::{Error, Result},
};

/// Report of actions taken by [`Workspace::sanitize`].
#[derive(Debug, Default)]
pub struct SanitizeReport {
    /// Conversations that were moved to `.trash/`.
    pub trashed: Vec<TrashedConversation>,

    /// Whether the active conversation was reassigned because the original was
    /// trashed or didn't exist.
    pub active_reassigned: bool,

    /// Whether all conversations were trashed (or none existed), causing the
    /// workspace to reset to fresh-workspace state.
    pub default_created: bool,
}

impl SanitizeReport {
    /// Returns `true` if any repairs were made.
    #[must_use]
    pub fn has_repairs(&self) -> bool {
        !self.trashed.is_empty() || self.active_reassigned || self.default_created
    }
}

/// A conversation that was moved to `.trash/` during sanitization.
#[derive(Debug)]
pub struct TrashedConversation {
    /// The original directory name.
    pub dirname: String,

    /// The reason this conversation was trashed.
    pub error: ValidationError,
}

impl Workspace {
    /// Validate and repair the conversations data store.
    ///
    /// Scans all conversation directories across both storage roots, trashes
    /// those that fail validation, and ensures the active conversation ID
    /// resolves to a valid conversation. Returns a report of what was repaired.
    ///
    /// This should be called before [`load_conversations_from_disk`] to
    /// guarantee the filesystem is in a consistent state.
    ///
    /// [`load_conversations_from_disk`]: Self::load_conversations_from_disk
    pub fn sanitize(&mut self) -> Result<SanitizeReport> {
        let storage = self.storage.as_ref().ok_or(Error::MissingStorage)?;

        let mut report = SanitizeReport::default();

        // Validate all conversation directories across both storage roots.
        let validation = storage.validate_conversations();

        let valid_ids: Vec<ConversationId> = validation.valid.iter().map(|v| v.id).collect();

        for entry in validation.invalid {
            trash_and_record(storage, entry, &mut report);
        }

        // Fresh workspace: no conversations on disk and nothing was trashed. We
        // don't need to parse the metadata — just remove any stale or corrupt
        // leftover so the filesystem is clean.
        if valid_ids.is_empty() && report.trashed.is_empty() {
            storage.remove_conversations_metadata()?;
            return Ok(report);
        }

        // Load the global conversations metadata, recovering from corruption.
        // Missing metadata is already handled by load_conversations_metadata
        // (returns default). Corrupt JSON is replaced with default here.
        let active_id = match storage.load_conversations_metadata() {
            Ok(m) => Some(m.active_conversation_id),
            Err(e) if e.is_corrupt() => {
                storage.remove_conversations_metadata()?;
                None
            }
            Err(e) => return Err(e.into()),
        };

        // If the active conversation is among the valid ones, we're done.
        if let Some(id) = active_id
            && valid_ids.contains(&id)
        {
            return Ok(report);
        }

        // The active conversation was trashed, never existed, or the metadata
        // was corrupt.
        report.active_reassigned = true;

        if let Some(&new_active) = valid_ids.iter().max() {
            storage.persist_conversations_metadata(&ConversationsMetadata::new(new_active))?;
        } else {
            // No valid conversations remain. Remove stale metadata so
            // `load_conversations_metadata` returns a fresh default.
            report.default_created = true;
            storage.remove_conversations_metadata()?;
        }

        Ok(report)
    }
}

/// Attempt to trash a conversation and record the action in the report.
fn trash_and_record(storage: &Storage, entry: InvalidConversation, report: &mut SanitizeReport) {
    if let Err(e) = storage.trash_conversation(&entry) {
        error!(
            dirname = entry.dirname,
            error = %entry.error,
            trash_error = %e,
            "Failed to trash conversation, skipping"
        );
        return;
    }

    report.trashed.push(TrashedConversation {
        dirname: entry.dirname.clone(),
        error: entry.error,
    });
}

#[cfg(test)]
#[path = "sanitize_tests.rs"]
mod tests;
