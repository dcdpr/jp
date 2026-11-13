//! Closing a prompt nobody is going to answer.
//!
//! A widget waits for a key, and two things end the wait without one:
//!
//! - [`close_all`] closes every prompt in the process.
//!   A program calls it when it is shutting down.
//! - A [`PromptCloser`] closes the prompts opened under it with [`closing_on`].
//!   A caller that opens a prompt on a thread of its own keeps the closer, and
//!   closes the prompt once it no longer needs the answer.
//!
//! Both are sticky: a prompt opened after the close returns at once, without
//! drawing.
//!
//! [`InlineReply`] closes.
//! A prompt the `inquire` crate draws does not, because `inquire` waits in a
//! read that only a key ends: [`InlineSelect`], and the widgets behind
//! [`PromptBackend::select`] and [`PromptBackend::password`].
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

/// How long a waiting widget goes without checking whether it is to close.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Set once every prompt in the process is to close.
static ALL_CLOSED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Closes the prompts this thread opens.
    static SCOPED: RefCell<Option<PromptCloser>> = const { RefCell::new(None) };
}

/// Closes the prompts opened under it with [`closing_on`].
///
/// Cloning gives another handle on the same prompts.
#[derive(Debug, Clone, Default)]
pub struct PromptCloser(Arc<AtomicBool>);

impl PromptCloser {
    /// A closer nothing has been opened under yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Close the open prompt, and every prompt opened under this closer from
    /// now on.
    pub fn close(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether [`Self::close`] was called.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Run `prompts`, closing every prompt it opens on this thread once `closer` is
/// closed.
///
/// A prompt opened on another thread is not covered.
pub fn closing_on<T>(closer: &PromptCloser, prompts: impl FnOnce() -> T) -> T {
    let _scope = Scope::enter(closer.clone());
    prompts()
}

/// Close every open prompt in the process, and every prompt opened from now on.
///
/// For a program that is shutting down: nothing reopens them.
pub fn close_all() {
    ALL_CLOSED.store(true, Ordering::Release);
}

/// Whether the prompt this thread has open, or is about to open, is to close.
///
/// What [`InlineReply`] watches while it waits for a key.
/// A [`PromptBackend`] of another kind polls it to close the same way.
///
/// [`InlineReply`]: crate::InlineReply
/// [`PromptBackend`]: crate::prompt::PromptBackend
#[must_use]
pub fn close_requested() -> bool {
    CloseWatch::current().requested()
}

/// What closes the prompts of the thread it was taken on.
///
/// Taken on the thread that opens the prompt, and polled from any thread.
#[derive(Debug, Clone)]
pub(crate) struct CloseWatch(Option<PromptCloser>);

impl CloseWatch {
    /// The closer this thread's prompts are opened under, if any.
    pub(crate) fn current() -> Self {
        Self(SCOPED.with(|scoped| scoped.borrow().clone()))
    }

    /// Whether the prompts are to close.
    pub(crate) fn requested(&self) -> bool {
        ALL_CLOSED.load(Ordering::Acquire) || self.0.as_ref().is_some_and(PromptCloser::is_closed)
    }
}

/// Puts back the closer a [`closing_on`] call replaced, when it ends.
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
