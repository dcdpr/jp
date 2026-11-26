//! Deduplication of provider notices across requests.
//!
//! A provider re-evaluates certain conditions on every request it sends, such
//! as a skipped credential, an exceeded quota threshold, or replayed reasoning
//! items that the API drops.
//! [`SeenNotices`] tracks which of these standing conditions have already been
//! reported so each is emitted at most once per provider instance.
//!
//! Event notices (such as a credential switch or a request billed as extra
//! usage) are emitted every time they occur and are not recorded in
//! [`SeenNotices`].

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};

/// Standing-condition keys for which a provider has already emitted a notice.
///
/// Clones share the underlying set across every stream opened by the provider.
#[derive(Debug, Clone, Default)]
pub(crate) struct SeenNotices(Arc<Mutex<HashSet<String>>>);

impl SeenNotices {
    /// Insert `key` and return `true` if it was not already present.
    ///
    /// `key` identifies the underlying condition rather than the formatted
    /// notice text, so notices whose wording varies (for example, by item ID or
    /// utilization percentage) can still be deduplicated by condition.
    pub(crate) fn first(&self, key: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_owned())
    }
}

#[cfg(test)]
#[path = "notices_tests.rs"]
mod tests;
