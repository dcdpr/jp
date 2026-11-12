//! Ordered queue of inbound message batches awaiting a turn or in-turn reply.
//!
//! Consecutive messages from the same sender in the same chat are coalesced
//! into a single [`Waiting`] batch; a message from a different sender starts a
//! new batch so each turn runs under its sender's identity.
//! While an in-turn reply is in flight, no other batch leaves the queue so a
//! rejected reply can be restored ahead of later messages.
//! Holding the queue (for example, while a conversation is being compacted)
//! prevents new turns from starting.

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

    /// Number of active holds (such as in-flight compactions) preventing new
    /// turns from starting.
    held: usize,

    /// Set once the bridge is stopping: nothing leaves the queue again.
    closed: bool,
}

impl Queue {
    /// Append `text` to the queue, coalescing with the last batch for `chat` if
    /// it is from `person`.
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

    /// Prepend a rejected in-turn reply ahead of any queued batches for `chat`.
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

    /// Return `waiting` to the front of the queue when its turn fails to start.
    pub(crate) fn restore(&mut self, waiting: Waiting) {
        self.batches.push_front(waiting);
    }

    /// Pop the next batch to start a turn, or `None` if the queue is closed,
    /// held, or awaiting a reply.
    pub(crate) fn next_turn(&mut self) -> Option<Waiting> {
        if self.closed || self.replying || self.held > 0 {
            return None;
        }

        self.batches.pop_front()
    }

    /// Pop the first batch for `chat` as an in-turn reply if it belongs to
    /// `person` and no reply is already in flight.
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

    /// Mark an in-turn reply as in flight for `chat` if no earlier batch for
    /// `chat` is queued.
    pub(crate) fn try_reply(&mut self, chat: &Chat) -> bool {
        if self.closed || self.replying || self.batches.iter().any(|waiting| waiting.chat == *chat)
        {
            return false;
        }

        self.replying = true;
        true
    }

    /// Clear the in-flight reply flag.
    pub(crate) fn replied(&mut self) {
        self.replying = false;
    }

    /// Increment the hold count, preventing new turns from starting.
    pub(crate) fn hold(&mut self) {
        self.held += 1;
    }

    /// Decrement the hold count.
    pub(crate) fn release(&mut self) {
        self.held = self.held.saturating_sub(1);
    }

    /// Close the queue so no further turns or replies are started.
    pub(crate) fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
