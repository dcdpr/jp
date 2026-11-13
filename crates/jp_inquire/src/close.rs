//! Programmatic closing for interactive prompts.
//!
//! Prompts can be closed without user input in two ways:
//!
//! - [`close_all`] closes every prompt in the process (used during graceful
//!   shutdown).
//! - [`PromptCloser`] closes prompts opened within a [`closing_on`] scope on
//!   the target thread (used when a caller no longer needs a prompt's answer).
//!
//! Both are sticky: a prompt opened after a close request returns immediately
//! without rendering.
//!
//! [`InlineReply`] supports programmatic closing.
//! Prompts rendered by the `inquire` crate ([`InlineSelect`],
//! [`PromptBackend::select`], and [`PromptBackend::password`]) block in a
//! synchronous key read and cannot be closed until the user presses a key.
//!
//! [`InlineReply`]: crate::InlineReply
//! [`InlineSelect`]: crate::InlineSelect
//! [`PromptBackend::password`]: crate::prompt::PromptBackend::password
//! [`PromptBackend::select`]: crate::prompt::PromptBackend::select

use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Polling interval for checking whether a prompt has been asked to close.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Process-wide flag set by [`close_all`].
static ALL_CLOSED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Thread-local [`PromptCloser`] installed by [`closing_on`].
    static SCOPED: RefCell<Option<PromptCloser>> = const { RefCell::new(None) };
}

/// Handle that closes prompts opened within a [`closing_on`] scope.
///
/// Cloning shares the same underlying flag.
#[derive(Debug, Clone, Default)]
pub struct PromptCloser(Arc<AtomicBool>);

impl PromptCloser {
    /// Create a new unclosed [`PromptCloser`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Close the active prompt and any subsequent prompts opened under this
    /// closer.
    pub fn close(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Return `true` if [`Self::close`] has been called.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Run `prompts` with `closer` installed on the current thread.
///
/// Prompts opened on other threads are unaffected.
pub fn closing_on<T>(closer: &PromptCloser, prompts: impl FnOnce() -> T) -> T {
    let _scope = Scope::enter(closer.clone());
    prompts()
}

/// Close all open and future prompts in the process.
pub fn close_all() {
    ALL_CLOSED.store(true, Ordering::Release);
}

/// Return `true` if the current thread's prompt scope or [`close_all`] has
/// requested a close.
///
/// Polled by [`InlineReply`] and custom [`PromptBackend`] implementations.
///
/// [`InlineReply`]: crate::InlineReply
/// [`PromptBackend`]: crate::prompt::PromptBackend
#[must_use]
pub fn close_requested() -> bool {
    CloseWatch::current().requested()
}

/// Snapshot of the current thread's [`PromptCloser`] that can be polled from
/// another thread.
#[derive(Debug, Clone)]
pub(crate) struct CloseWatch(Option<PromptCloser>);

impl CloseWatch {
    /// Capture the current thread's [`PromptCloser`], if any.
    pub(crate) fn current() -> Self {
        Self(SCOPED.with(|scoped| scoped.borrow().clone()))
    }

    /// Return `true` if [`close_all`] or the captured [`PromptCloser`] has
    /// fired.
    pub(crate) fn requested(&self) -> bool {
        ALL_CLOSED.load(Ordering::Acquire) || self.0.as_ref().is_some_and(PromptCloser::is_closed)
    }
}

/// Guard that restores the previous thread-local [`PromptCloser`] on drop.
struct Scope(Option<PromptCloser>);

impl Scope {
    fn enter(closer: PromptCloser) -> Self {
        Self(SCOPED.with(|scoped| scoped.replace(Some(closer))))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let outer = self.0.take();
        SCOPED.with(|scoped| *scoped.borrow_mut() = outer);
    }
}

#[cfg(test)]
#[path = "close_tests.rs"]
mod tests;
