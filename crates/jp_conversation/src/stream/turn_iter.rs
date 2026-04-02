//! Turn-level iteration over a [`ConversationStream`].
//!
//! A "turn" is a group of events delimited by [`TurnStart`] markers. Events
//! before the first `TurnStart` (if any) form an implicit leading turn.
//!
//! [`TurnStart`]: crate::event::TurnStart

use super::ConversationEventWithConfigRef;

/// A group of events belonging to a single turn in the conversation.
///
/// Each turn starts with a [`TurnStart`] event (except possibly the first
/// implicit turn) followed by the chat requests, responses, tool calls, etc.
/// that make up that turn.
///
/// [`TurnStart`]: crate::event::TurnStart
#[derive(Debug)]
pub struct Turn<'a> {
    /// The events in this turn, including the leading `TurnStart` (if present).
    events: Vec<ConversationEventWithConfigRef<'a>>,
}

impl<'a> Turn<'a> {
    /// Iterate over the events in this turn.
    pub fn iter(&self) -> std::slice::Iter<'_, ConversationEventWithConfigRef<'a>> {
        self.events.iter()
    }
}

impl<'a> IntoIterator for Turn<'a> {
    type Item = ConversationEventWithConfigRef<'a>;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.into_iter()
    }
}

impl<'a, 'b> IntoIterator for &'b Turn<'a> {
    type Item = &'b ConversationEventWithConfigRef<'a>;
    type IntoIter = std::slice::Iter<'b, ConversationEventWithConfigRef<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter()
    }
}

/// An iterator over the turns in a conversation stream.
///
/// Created by [`ConversationStream::iter_turns`].
///
/// [`ConversationStream::iter_turns`]: super::ConversationStream::iter_turns
pub struct IterTurns<'a>(std::vec::IntoIter<Turn<'a>>);

impl<'a> IterTurns<'a> {
    /// Build an [`IterTurns`] from the stream's event iterator.
    pub(super) fn new(events: impl Iterator<Item = ConversationEventWithConfigRef<'a>>) -> Self {
        let mut turns: Vec<Turn<'a>> = Vec::new();
        let mut current: Vec<ConversationEventWithConfigRef<'a>> = Vec::new();

        for event in events {
            if event.event.is_turn_start() && !current.is_empty() {
                turns.push(Turn { events: current });
                current = Vec::new();
            }
            current.push(event);
        }

        if !current.is_empty() {
            turns.push(Turn { events: current });
        }

        Self(turns.into_iter())
    }
}

impl<'a> Iterator for IterTurns<'a> {
    type Item = Turn<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for IterTurns<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back()
    }
}

impl ExactSizeIterator for IterTurns<'_> {}

#[cfg(test)]
#[path = "turn_iter_tests.rs"]
mod tests;
