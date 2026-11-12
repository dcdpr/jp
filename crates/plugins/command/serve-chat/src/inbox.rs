//! Ordered buffer of inbound messages awaiting pre-turn classification.
//!
//! When a [`Decide`] client is configured, messages are classified one at a
//! time in arrival order so later messages cannot overtake earlier ones.
//! [`Inbox`] also tracks recent chat lines for classification context and the
//! set of unconfigured sender IDs that have already been reported.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use crate::{
    decide::{Action, Decide, Decision},
    transport::Chat,
};

/// Maximum number of recent lines retained per chat for decision context.
const RECENT_LINES: usize = 8;

/// Maximum characters per line retained in decision context.
const MAX_LINE: usize = 500;

/// Inbound message buffered in the [`Inbox`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Incoming {
    pub chat: Chat,

    /// Key of the sender in `options.people`.
    pub person: String,

    /// Formatted message text, prefixed with the sender's name in group chats.
    pub text: String,

    /// Whether the message can issue a command (sent in a direct chat, or
    /// starting with a bot mention in a group).
    pub commands: bool,

    /// Recent chat transcript passed to the decider, if configured.
    pub context: Option<String>,
}

/// Next step for the front message in the [`Inbox`].
pub(crate) enum Next {
    /// Route the message immediately.
    Take(Incoming),

    /// Await this decision for the front message, then call [`Inbox::decided`].
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
    pub(crate) fn decide_with(&mut self, decider: Option<Arc<dyn Decide>>) {
        self.decider = decider;
    }

    /// Record `id` as seen and return `true` if this is the first sighting.
    pub(crate) fn first_sighting(&mut self, id: &str) -> bool {
        self.strangers.insert(id.to_owned())
    }

    /// Append `line` to the recent transcript for `chat`.
    pub(crate) fn remember(&mut self, chat: &Chat, line: &str) {
        let lines = self.recent.entry(chat.clone()).or_default();
        lines.push_back(line.chars().take(MAX_LINE).collect());
        while lines.len() > RECENT_LINES {
            lines.pop_front();
        }
    }

    /// Enqueue a message from `person` in `chat`.
    pub(crate) fn push(&mut self, chat: Chat, person: String, text: String, commands: bool) {
        let context = self.decider.is_some().then(|| self.context(&chat, &text));
        self.remember(&chat, &text);
        self.pending.push_back(Incoming {
            chat,
            person,
            text,
            commands,
            context,
        });
    }

    /// Return the next action for the front message, or `None` if empty or a
    /// decision is already in flight.
    pub(crate) fn next(&mut self) -> Option<Next> {
        if self.deciding {
            return None;
        }
        let front = self.pending.front()?;

        match (&front.context, &self.decider) {
            (Some(context), Some(decider)) => {
                self.deciding = true;
                let actions = Action::offered(front.commands);
                Some(Next::Decide(decider.decide(context.clone(), actions)))
            }
            _ => self.pending.pop_front().map(Next::Take),
        }
    }

    /// Pop and return the message whose in-flight decision just completed.
    pub(crate) fn decided(&mut self) -> Option<Incoming> {
        self.deciding = false;
        self.pending.pop_front()
    }

    /// Format the decision prompt state for `text` using recent lines from
    /// `chat`.
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
