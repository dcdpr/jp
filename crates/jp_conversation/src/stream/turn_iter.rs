//! Turn-level iteration over a [`ConversationStream`].
//!
//! A "turn" is a group of events delimited by [`TurnStart`] markers.
//! Events before the first `TurnStart` (if any) form an implicit leading turn.
//!
//! [`ConversationStream`]: super::ConversationStream
//! [`TurnStart`]: crate::event::TurnStart

use super::ConversationEventWithConfigRef;
use crate::{EventId, event::format_turn_summary};

/// A group of events belonging to a single turn in the conversation.
///
/// Each turn starts with a [`TurnStart`] event (except possibly the first
/// implicit turn) followed by the chat requests, responses, tool calls, etc.
/// that make up that turn.
///
/// [`TurnStart`]: crate::event::TurnStart
#[derive(Debug)]
pub struct Turn<'a> {
    /// The 0-based index of this turn in the conversation.
    index: usize,
    /// The events in this turn, including the leading `TurnStart` (if present).
    events: Vec<ConversationEventWithConfigRef<'a>>,
}

impl<'a> Turn<'a> {
    /// The 0-based index of this turn in the conversation.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// Iterate over the events in this turn.
    pub fn iter(&self) -> std::slice::Iter<'_, ConversationEventWithConfigRef<'a>> {
        self.events.iter()
    }

    /// The [`EventId`] of this turn's leading [`TurnStart`] entry.
    ///
    /// Returns `None` for an implicit leading turn whose events precede the
    /// first `TurnStart`.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    #[must_use]
    pub fn id(&self) -> Option<&'a EventId> {
        self.events
            .first()
            .filter(|event| event.is_turn_start())
            .map(|event| event.event_id)
    }

    /// The summary stored on this turn's [`TurnStart`], if present.
    ///
    /// Always `None` for an implicit leading turn.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    #[must_use]
    pub fn summary(&self) -> Option<&'a str> {
        self.events
            .first()
            .and_then(|event| event.event.as_turn_start())
            .and_then(|turn_start| turn_start.summary.as_deref())
    }

    /// Fallback summary derived from the first non-empty line of the turn's
    /// first [`ChatRequest`].
    ///
    /// Collapses whitespace to single spaces and truncates to
    /// [`MAX_SUMMARY_CHARS`] characters.
    /// Returns `None` if the turn has no non-empty `ChatRequest`.
    ///
    /// [`ChatRequest`]: crate::event::ChatRequest
    /// [`MAX_SUMMARY_CHARS`]: crate::event::MAX_SUMMARY_CHARS
    #[must_use]
    pub fn fallback_summary(&self) -> Option<String> {
        let request = self.iter().find_map(|event| event.as_chat_request())?;
        let line = request
            .content
            .lines()
            .find(|line| !line.trim().is_empty())?;

        format_turn_summary(line)
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
                let index = turns.len();
                turns.push(Turn {
                    index,
                    events: current,
                });
                current = Vec::new();
            }
            current.push(event);
        }

        if !current.is_empty() {
            let index = turns.len();
            turns.push(Turn {
                index,
                events: current,
            });
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
