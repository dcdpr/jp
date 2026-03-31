use jp_storage::{
    Storage,
    validate::{InvalidConversation, ValidationError},
};
use tracing::{error, trace};

use crate::{
    Workspace,
    error::{Error, Result},
};

/// Report of actions taken by [`Workspace::sanitize`].
#[derive(Debug, Default)]
pub struct SanitizeReport {
    /// Conversations that were moved to `.trash/`.
    pub trashed: Vec<TrashedConversation>,
}

impl SanitizeReport {
    /// Returns `true` if any repairs were made.
    #[must_use]
    pub fn has_repairs(&self) -> bool {
        !self.trashed.is_empty()
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
    /// those that fail validation. Returns a report of what was repaired.
    ///
    /// This should be called before [`load_conversation_index`] to
    /// guarantee the filesystem is in a consistent state.
    ///
    /// [`load_conversation_index`]: Self::load_conversation_index
    pub fn sanitize(&mut self) -> Result<SanitizeReport> {
        trace!("Sanitizing workspace.");

        let storage = self.storage.as_ref().ok_or(Error::MissingStorage)?;

        let mut report = SanitizeReport::default();

        let validation = storage.validate_conversations();

        for entry in validation.invalid {
            trash_and_record(storage, entry, &mut report);
        }

        trace!(trashed = report.trashed.len(), "Sanitization complete.",);

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
