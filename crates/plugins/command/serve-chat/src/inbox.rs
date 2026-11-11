//! Messages from configured people, in the order they arrived, until the bridge
//! takes them.
//!
//! With a decider configured, each message waits for the decider to say what
//! the bridge does with it.
//! Every message behind it waits too, so the bridge takes a chat's messages in
//! the order they were sent.
//!
//! The inbox also keeps each chat's last few lines, which a decision reads as
//! context, and the senders the bridge has already reported as unknown.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use crate::{
    decide::{Decide, Decision},
    transport::Chat,
};

/// How many recent lines of a chat a decision reads.
const RECENT_LINES: usize = 8;

/// The most characters of one line a decision reads.
const MAX_LINE: usize = 500;

/// A message on its way to the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Incoming {
    pub chat: Chat,

    /// Key of the sender in `options.people`.
    pub person: String,

    /// What the turn reads, with the sender's name in a group.
    pub text: String,

    /// Whether it was sent to the assistant: in a direct chat, or mentioning it
    /// or replying to it in a group.
    pub addressed: bool,

    /// What a decision about it reads, for a message that waits for one.
    pub context: Option<String>,
}

/// What the bridge does next with the inbox.
pub(crate) enum Next {
    /// Take this message.
    Take(Incoming),

    /// Wait for this decision about the first message, and pass its result to
    /// [`Inbox::decided`].
    Decide(Decision),
}

#[derive(Default)]
pub(crate) struct Inbox {
    pending: VecDeque<Incoming>,

    /// Whether a decision about the first pending message is in flight.
    deciding: bool,

    recent: HashMap<Chat, VecDeque<String>>,

    /// Primary sender IDs of unconfigured senders already reported.
    strangers: HashSet<String>,

    decider: Option<Arc<dyn Decide>>,
}

impl Inbox {
    /// Ask `decider` what to do with each message.
    pub(crate) fn decide_with(&mut self, decider: Option<Arc<dyn Decide>>) {
        self.decider = decider;
    }

    /// Record `id` as reported, returning whether it wasn't before.
    pub(crate) fn first_sighting(&mut self, id: &str) -> bool {
        self.strangers.insert(id.to_owned())
    }

    /// Add `line` to what was said in `chat`.
    pub(crate) fn remember(&mut self, chat: &Chat, line: &str) {
        let lines = self.recent.entry(chat.clone()).or_default();
        lines.push_back(line.chars().take(MAX_LINE).collect());
        while lines.len() > RECENT_LINES {
            lines.pop_front();
        }
    }

    /// Queue `text` from `person` in `chat`, to wait for a decision when there
    /// is a decider.
    pub(crate) fn push(&mut self, chat: Chat, person: String, text: String, addressed: bool) {
        let context = self.decider.is_some().then(|| self.context(&chat, &text));
        self.remember(&chat, &text);
        self.pending.push_back(Incoming {
            chat,
            person,
            text,
            addressed,
            context,
        });
    }

    /// What to do next: take the first message, or wait for a decision about
    /// it.
    ///
    /// `None` while a decision is in flight, or nothing is waiting.
    pub(crate) fn next(&mut self) -> Option<Next> {
        if self.deciding {
            return None;
        }
        let front = self.pending.front()?;

        match (&front.context, &self.decider) {
            (Some(context), Some(decider)) => {
                self.deciding = true;
                Some(Next::Decide(decider.decide(context.clone())))
            }
            _ => self.pending.pop_front().map(Next::Take),
        }
    }

    /// The message the decision in flight was about.
    pub(crate) fn decided(&mut self) -> Option<Incoming> {
        self.deciding = false;
        self.pending.pop_front()
    }

    /// What a decision about `text` in `chat` reads: the lines before it, then
    /// it.
    fn context(&self, chat: &Chat, text: &str) -> String {
        let mut context = String::new();
        if let Some(lines) = self.recent.get(chat).filter(|lines| !lines.is_empty()) {
            context.push_str("Earlier messages, oldest first:\n");
            for line in lines {
                context.push_str(line);
                context.push('\n');
            }
            context.push('\n');
        }
        context.push_str("Last message:\n");
        context.push_str(text);

        context
    }
}

#[cfg(test)]
#[path = "inbox_tests.rs"]
mod tests;
