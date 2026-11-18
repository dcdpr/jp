pub use jp_storage::backend::SanitizeReport;
use tracing::trace;

use crate::{Workspace, error::Result};

impl Workspace {
    /// Check the conversations data store and report the conversations that
    /// cannot be loaded.
    ///
    /// Delegates to [`LoadBackend::sanitize`], which never moves or deletes a
    /// conversation.
    /// The conversations it reports are also left out of
    /// [`load_conversation_index`].
    ///
    /// [`LoadBackend::sanitize`]: jp_storage::backend::LoadBackend::sanitize
    /// [`load_conversation_index`]: Self::load_conversation_index
    pub fn sanitize(&mut self) -> Result<SanitizeReport> {
        trace!("Sanitizing workspace.");

        let report = self.loader.sanitize()?;

        trace!(invalid = report.invalid.len(), "Sanitization complete.");
        Ok(report)
    }
}

#[cfg(test)]
#[path = "sanitize_tests.rs"]
mod tests;
