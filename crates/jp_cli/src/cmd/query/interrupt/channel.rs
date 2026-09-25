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
//!
//! Every interrupt carries an acknowledgement.
//! It resolves when the turn takes the interrupt to act on it, and fails when
//! the turn ends without doing so, so a sender learns whether its interrupt
//! reached the conversation rather than only whether it was queued.

use std::{
    collections::VecDeque,
    task::{Context, Poll},
};

use tokio::sync::{
    mpsc::{self, error::TrySendError},
    oneshot,
};

use super::InterruptAction;

/// How many undelivered interrupts a turn will hold.
///
/// A turn consumes these between provider events, so the queue only builds up
/// while one is in flight.
/// Small on purpose: a client that has sent eight unanswered interrupts is not
/// going to be helped by a ninth being accepted.
const CAPACITY: usize = 8;

/// An interrupt on its way to a turn, with the means to tell its sender the
/// turn took it.
struct ClientInterrupt {
    action: InterruptAction,
    taken: oneshot::Sender<()>,
}

impl ClientInterrupt {
    /// Hand over the action, telling the sender it reached the turn.
    fn take(self) -> InterruptAction {
        // A sender that stopped waiting changes nothing about what the turn does
        // with the action.
        self.taken.send(()).ok();
        self.action
    }

    /// Whether this interrupt ends a turn that has not started yet.
    fn ends_the_turn(&self) -> bool {
        matches!(self.action, InterruptAction::Stop | InterruptAction::Abort)
    }
}

/// Resolves once the turn has taken an interrupt to act on it.
///
/// Fails when the turn ends without taking it.
pub(crate) type Delivery = oneshot::Receiver<()>;

/// Sends interrupts to one running turn.
///
/// Cloneable, and every clone reaches the same turn.
#[derive(Clone)]
pub(crate) struct TurnInterruptSender(mpsc::Sender<ClientInterrupt>);

impl TurnInterruptSender {
    /// Queue `action` for the turn.
    ///
    /// Queued is not delivered: the returned [`Delivery`] is what says whether
    /// the turn acted on it.
    /// Fails straight away when the turn has already stopped reading, or has
    /// fallen behind by [`CAPACITY`] interrupts.
    pub(crate) fn try_send(
        &self,
        action: InterruptAction,
    ) -> Result<Delivery, TrySendError<InterruptAction>> {
        let (taken, delivery) = oneshot::channel();

        self.0
            .try_send(ClientInterrupt { action, taken })
            .map_err(|error| match error {
                TrySendError::Full(interrupt) => TrySendError::Full(interrupt.action),
                TrySendError::Closed(interrupt) => TrySendError::Closed(interrupt.action),
            })?;

        Ok(delivery)
    }
}

/// Receives interrupts aimed at this turn.
///
/// Taking an interrupt from here tells its sender it was delivered, so an
/// interrupt is only taken where it is acted on.
/// Dropping this refuses everything still waiting.
pub(crate) struct TurnInterrupts {
    rx: mpsc::Receiver<ClientInterrupt>,

    /// Interrupts that arrived before the turn could act on them, oldest first,
    /// and are handed out ahead of the channel.
    held: VecDeque<ClientInterrupt>,
}

impl TurnInterrupts {
    /// Create both ends of a turn's interrupt channel.
    pub(crate) fn channel() -> (TurnInterruptSender, Self) {
        let (tx, rx) = mpsc::channel(CAPACITY);
        (TurnInterruptSender(tx), Self {
            rx,
            held: VecDeque::new(),
        })
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
        Self {
            rx,
            held: VecDeque::new(),
        }
    }

    /// Take the next interrupt if one has already arrived.
    ///
    /// Never waits.
    pub(crate) fn try_next(&mut self) -> Option<InterruptAction> {
        self.held
            .pop_front()
            .or_else(|| self.rx.try_recv().ok())
            .map(ClientInterrupt::take)
    }

    /// Wait for the next interrupt.
    ///
    /// Yields `None` once every sender is gone, which for
    /// [`TurnInterrupts::none`] is immediately.
    /// Cancel-safe, so it can be raced against other work.
    pub(crate) async fn next(&mut self) -> Option<InterruptAction> {
        if let Some(interrupt) = self.held.pop_front() {
            return Some(interrupt.take());
        }

        self.rx.recv().await.map(ClientInterrupt::take)
    }

    /// Poll for the next interrupt.
    ///
    /// Yields `Ready(None)` once every sender is gone, which for
    /// [`TurnInterrupts::none`] is immediately.
    pub(crate) fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<InterruptAction>> {
        if let Some(interrupt) = self.held.pop_front() {
            return Poll::Ready(Some(interrupt.take()));
        }

        self.rx
            .poll_recv(cx)
            .map(|next| next.map(ClientInterrupt::take))
    }

    /// Wait for an interrupt that ends the turn before it has started.
    ///
    /// For the spans before anything is appended to the conversation.
    /// A stop or an abort is taken and returned; a reply cannot be acted on
    /// until the turn's own request is in place, so it is held back for
    /// [`next`] and the others to hand out once it is.
    ///
    /// Yields `None` once every sender is gone.
    /// Cancel-safe: an interrupt received is either returned or held before the
    /// future can be dropped.
    ///
    /// [`next`]: Self::next
    pub(crate) async fn next_stop(&mut self) -> Option<InterruptAction> {
        loop {
            let interrupt = self.rx.recv().await?;
            if interrupt.ends_the_turn() {
                return Some(interrupt.take());
            }

            self.held.push_back(interrupt);
        }
    }
}

#[cfg(test)]
#[path = "channel_tests.rs"]
mod tests;
