//! Messages waiting for a turn, kept in the order they were sent.
//!
//! Consecutive messages from one person in one chat form one batch, which
//! becomes one query; a different person's message starts a batch of its own,
//! so a turn runs as whoever asked for it.
//! While a reply is in flight to a running turn, nothing leaves the queue: if
//! the turn refuses the reply, it goes back ahead of the messages that came
//! after it.
//! While the queue is held, such as during a compaction, no turn starts.

use std::collections::VecDeque;

use crate::transport::Chat;

/// Consecutive queued messages from one person in one chat, awaiting a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Waiting {
    pub chat: Chat,

    /// Key of the sender in `options.people`, used as the turn's identity.
    pub person: String,

    pub texts: Vec<String>,
}

impl Waiting {
    /// A batch holding the one message `text`.
    fn new(chat: Chat, person: String, text: String) -> Self {
        Self {
            chat,
            person,
            texts: vec![text],
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Queue {
    batches: VecDeque<Waiting>,

    /// Whether a reply is in flight to the running turn.
    replying: bool,

    /// How many things other than a turn, such as compactions, are using a
    /// conversation; no turn starts until none is.
    held: usize,

    /// Set once the bridge is stopping: nothing leaves the queue again.
    closed: bool,
}

impl Queue {
    /// Queue `text` after everything waiting, joining the last batch for `chat`
    /// if `person` wrote it.
    pub(crate) fn push(&mut self, chat: Chat, person: String, text: String) {
        match self
            .batches
            .iter()
            .rposition(|waiting| waiting.chat == chat)
        {
            Some(index) if self.batches[index].person == person => {
                self.batches[index].texts.push(text);
            }
            _ => self.batches.push_back(Waiting::new(chat, person, text)),
        }
    }

    /// Queue `text` ahead of everything waiting for `chat`, for a reply a turn
    /// didn't take: it was sent before them.
    pub(crate) fn push_first(&mut self, chat: Chat, person: String, text: String) {
        let index = self.batches.iter().position(|queued| queued.chat == chat);
        match index {
            Some(index) if self.batches[index].person == person => {
                self.batches[index].texts.insert(0, text);
            }
            // Ahead of the chat's first batch, or of everything when it has none.
            _ => self
                .batches
                .insert(index.unwrap_or(0), Waiting::new(chat, person, text)),
        }
    }

    /// Put back a batch whose turn couldn't start, at the head of the queue.
    pub(crate) fn restore(&mut self, waiting: Waiting) {
        self.batches.push_front(waiting);
    }

    /// The next batch to run as a turn, unless a reply is in flight, the queue
    /// is held, or it is closed.
    pub(crate) fn next_turn(&mut self) -> Option<Waiting> {
        if self.closed || self.replying || self.held > 0 {
            return None;
        }

        self.batches.pop_front()
    }

    /// Take the first batch waiting for `chat` as the reply in flight, if
    /// `person` wrote it and no reply is in flight already: what a turn running
    /// as `person` in `chat` takes next.
    pub(crate) fn next_reply(&mut self, chat: &Chat, person: &str) -> Option<Waiting> {
        if self.closed || self.replying {
            return None;
        }
        let index = self
            .batches
            .iter()
            .position(|waiting| waiting.chat == *chat)?;
        if self.batches[index].person != person {
            return None;
        }

        self.replying = true;
        self.batches.remove(index)
    }

    /// Claim the reply in flight for a message just sent in `chat`, if it can
    /// go into the running turn without overtaking an earlier one.
    ///
    /// Returns whether it was claimed; a message that can't be sent now is for
    /// [`Self::push`].
    pub(crate) fn try_reply(&mut self, chat: &Chat) -> bool {
        if self.closed || self.replying || self.batches.iter().any(|waiting| waiting.chat == *chat)
        {
            return false;
        }

        self.replying = true;
        true
    }

    /// Record that the reply in flight was answered, which lets the queue go
    /// on.
    pub(crate) fn replied(&mut self) {
        self.replying = false;
    }

    /// Start no turn until a matching [`Self::release`].
    pub(crate) fn hold(&mut self) {
        self.held += 1;
    }

    /// Let turns start again once every [`Self::hold`] is released.
    pub(crate) fn release(&mut self) {
        self.held = self.held.saturating_sub(1);
    }

    /// Release nothing from now on.
    pub(crate) fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
