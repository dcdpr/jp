// Re-export from jp_storage::backend for backward compatibility.
pub use jp_storage::backend::{SanitizeReport, TrashedConversation};
use tracing::trace;

use crate::{Workspace, error::Result};

impl Workspace {
    /// Validate and repair the conversations data store.
    ///
    /// Delegates to [`LoadBackend::sanitize`], which scans all conversation
    /// directories (for filesystem backends), trashes those that fail
    /// validation, and returns a report. For in-memory backends, this
    /// returns an empty report.
    ///
    /// Call this before [`load_conversation_index`] to guarantee the backing
    /// store is in a consistent state.
    ///
    /// [`LoadBackend::sanitize`]: jp_storage::backend::LoadBackend::sanitize
    /// [`load_conversation_index`]: Self::load_conversation_index
    pub fn sanitize(&mut self) -> Result<SanitizeReport> {
        trace!("Sanitizing workspace.");

        let report = self.loader.sanitize()?;

        trace!(trashed = report.trashed.len(), "Sanitization complete.");
        Ok(report)
    }
}

#[cfg(test)]
#[path = "sanitize_tests.rs"]
mod tests;
