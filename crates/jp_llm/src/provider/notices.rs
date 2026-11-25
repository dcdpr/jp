//! Deduplication of the notices a provider yields.
//!
//! A provider derives some notices again on every request it sends: a
//! credential skipped for the same reason, a quota threshold already crossed,
//! replayed reasoning the API withholds again.
//! [`SeenNotices`] records which of those a provider has yielded, so each is
//! yielded once per provider.
//!
//! The provider decides which notices restate a standing condition.
//! A notice that reports an event (a credential switch, a request billed as
//! extra usage) is yielded every time it happens and is never recorded here.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};

/// The standing conditions one provider has already yielded a notice for.
///
/// Clones share one record, so every stream a provider opens reads and writes
/// the same one.
#[derive(Debug, Clone, Default)]
pub(crate) struct SeenNotices(Arc<Mutex<HashSet<String>>>);

impl SeenNotices {
    /// Record `key`, returning whether it had not been seen before.
    ///
    /// `key` identifies the condition, which need not be the notice's text: a
    /// notice whose wording changes while the condition stands is keyed on what
    /// stays the same.
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
