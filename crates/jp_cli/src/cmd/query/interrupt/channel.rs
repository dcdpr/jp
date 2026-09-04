//! Interrupts aimed at a running turn from outside the process.
//!
//! A turn driven from a terminal learns what the user wants through its own
//! prompt: Ctrl-C notifies the loop, the loop shows the interrupt menu, and the
//! menu returns an [`InterruptAction`].
//! A turn nobody is sitting in front of has no menu to show, so the action
//! arrives already decided, over the channel in this module.
//!
//! [`TurnInterrupts`] is the receiving end, owned by the turn.
//! [`TurnInterruptSender`] is the sending end, held by whoever is driving that
//! turn from elsewhere.

use std::task::{Context, Poll};

use tokio::sync::mpsc;

use super::InterruptAction;

/// How many undelivered interrupts a turn will hold.
///
/// A turn consumes these between provider events, so the queue only builds up
/// while one is in flight.
/// Small on purpose: a client that has sent eight unanswered interrupts is not
/// going to be helped by a ninth being accepted.
const CAPACITY: usize = 8;

/// Sends interrupts to one running turn.
///
/// Cloneable, and every clone reaches the same turn.
/// Sending fails once the turn has ended, which is how a client learns there
/// was nothing left to interrupt.
pub(crate) type TurnInterruptSender = mpsc::Sender<InterruptAction>;

/// Receives interrupts aimed at this turn.
pub(crate) struct TurnInterrupts(mpsc::Receiver<InterruptAction>);

impl TurnInterrupts {
    /// Create both ends of a turn's interrupt channel.
    pub(crate) fn channel() -> (TurnInterruptSender, Self) {
        let (tx, rx) = mpsc::channel(CAPACITY);
        (tx, Self(rx))
    }

    /// An end nothing can send to.
    ///
    /// For a turn whose only user is at the terminal it is running in: the
    /// keyboard reaches it through the signal router, and no client holds a
    /// sender.
    /// Reads as an ended stream, so a caller polling it alongside other sources
    /// drops it on the first poll.
    pub(crate) fn none() -> Self {
        let (_, rx) = mpsc::channel(1);
        Self(rx)
    }

    /// Take the next interrupt if one has already arrived.
    ///
    /// Never waits.
    pub(crate) fn try_next(&mut self) -> Option<InterruptAction> {
        self.0.try_recv().ok()
    }

    /// Wait for the next interrupt.
    ///
    /// Yields `None` once every sender is gone, which for
    /// [`TurnInterrupts::none`] is immediately.
    /// Cancel-safe, so it can be raced against other work.
    pub(crate) async fn next(&mut self) -> Option<InterruptAction> {
        self.0.recv().await
    }

    /// Poll for the next interrupt.
    ///
    /// Yields `Ready(None)` once every sender is gone, which for
    /// [`TurnInterrupts::none`] is immediately.
    pub(crate) fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<InterruptAction>> {
        self.0.poll_recv(cx)
    }
}
