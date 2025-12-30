use std::collections::VecDeque;

use jp_conversation::{ConversationEvent, event::ChatResponse};
use tracing::debug;

use crate::event::{Event, FinishReason};

/// A stateful processor that manages chaining multiple streams together.
///
/// It buffers events to handle smooth merging of content when a stream ends
/// with [`FinishReason::MaxTokens`].
#[derive(Debug)]
pub struct EventChain {
    /// Events from the current stream that are buffered.
    ///
    /// In `Normal` state, this holds the "tail" of the stream that we keep
    /// around to check for overlaps if we hit a `MaxTokens`.
    ///
    /// In `Merging` state, this holds the "tail" of the *previous* stream.
    buffer: VecDeque<Event>,

    /// Events from the *next* stream that we are accumulating until we can
    /// determine the merge point.
    pending: VecDeque<Event>,

    /// State of the chain.
    state: ChainState,

    /// Number of characters to keep in the buffer for overlap checking.
    min_overlap: usize,

    /// Maximum characters to look back for merging.
    max_overlap: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum ChainState {
    /// Normal streaming mode.
    Normal,

    /// We hit `MaxTokens` and are waiting for enough content from the new
    /// stream to merge.
    Merging,
}

impl Default for EventChain {
    fn default() -> Self {
        Self::new()
    }
}

impl EventChain {
    /// Create a new event chain.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: VecDeque::new(),
            pending: VecDeque::new(),
            state: ChainState::Normal,
            min_overlap: 20,
            max_overlap: 500,
        }
    }

    /// Set the minimum number of characters required to confirm an overlap.
    #[must_use]
    pub fn with_min_overlap(mut self, min_overlap: usize) -> Self {
        self.min_overlap = min_overlap;
        self
    }

    /// Set the maximum number of characters to retain in the buffer for overlap
    /// checking.
    #[must_use]
    pub fn with_max_overlap(mut self, max_overlap: usize) -> Self {
        self.max_overlap = max_overlap;
        self
    }

    /// Ingest a stream event.
    ///
    /// Returns a vector of events ready to be emitted.
    ///
    /// If the input event is `Finished(MaxTokens)`, it is consumed, the chain
    /// enters "merge mode", and the caller is expected to start a new stream
    /// and feed its events into this method.
    pub fn ingest(&mut self, event: Event) -> Vec<Event> {
        match self.state {
            ChainState::Normal => self.ingest_normal(event),
            ChainState::Merging => self.ingest_merging(event),
        }
    }

    fn ingest_normal(&mut self, event: Event) -> Vec<Event> {
        match event {
            // If we hit MaxTokens, we swallow the event and switch state.
            //
            // We do NOT emit the buffer yet; we keep it to merge with the next
            // stream.
            Event::Finished(FinishReason::MaxTokens) => {
                debug!("MaxTokens received, switching to Merging state.");
                self.state = ChainState::Merging;
                vec![]
            }

            // If the stream finishes normally, we flush everything.
            Event::Finished(reason) => {
                let mut out = Vec::with_capacity(self.buffer.len() + 1);
                out.extend(self.buffer.drain(..));
                out.push(Event::Finished(reason));
                out
            }

            // Individual parts or flushes are buffered, but we trim the buffer
            // from the start if we exceed the max overlap.
            Event::Part { .. } | Event::Flush { .. } => {
                self.buffer.push_back(event);
                self.trim_buffer()
            }
        }
    }

    fn ingest_merging(&mut self, event: Event) -> Vec<Event> {
        match event {
            // If the *new* stream finishes while we are still trying to find a
            // merge, we have to give up on merging. We flush the old buffer,
            // then the pending buffer, then the finish.
            Event::Finished(reason) => {
                debug!("New stream finished while merging. Trying to merge with less data.");

                // Try to merge with whatever we have, even if it's less than
                // min_overlap. We'll try progressively smaller overlaps.
                //
                // Note: We use 3 as the absolute minimum overlap to merge. If
                // even 3 chars don't match, we assume no overlap.
                let merged_events = if self.pending.is_empty() {
                    vec![]
                } else {
                    self.attempt_merge(3)
                };

                let mut out = Vec::with_capacity(
                    merged_events.len() + self.buffer.len() + self.pending.len() + 1,
                );

                // If merge succeeded, buffer is already drained. If merge
                // failed, we drain buffer manually.
                if merged_events.is_empty() {
                    out.extend(self.buffer.drain(..));
                    out.extend(self.pending.drain(..));
                } else {
                    out.extend(merged_events);
                }

                out.push(Event::Finished(reason));
                self.state = ChainState::Normal;
                out
            }

            Event::Part { .. } | Event::Flush { .. } => {
                self.pending.push_back(event);
                self.attempt_merge(self.min_overlap)
            }
        }
    }

    /// Emits events from the front of the buffer to keep it within size limits.
    ///
    /// We always want to keep the *end* of the stream in the buffer.
    fn trim_buffer(&mut self) -> Vec<Event> {
        let current_len = self.buffer_text_len();

        // If we are below the max overlap, we don't emit anything yet to ensure
        // we have enough context.
        if current_len <= self.max_overlap {
            return vec![];
        }

        // Calculate how much text we need to drop to get back to max_overlap.
        let mut to_remove_len = current_len - self.max_overlap;
        let mut emit = Vec::new();

        while let Some(evt) = self.buffer.front() {
            let evt_len = event_text_len(evt);

            // If removing this event keeps us above or near the target, remove it.
            if to_remove_len == 0 {
                break;
            }

            if evt_len <= to_remove_len {
                // Safe to remove whole event
                to_remove_len -= evt_len;
                emit.push(self.buffer.pop_front().unwrap());
            } else {
                // The next event contains the boundary. Stop here.
                break;
            }
        }

        emit
    }

    fn buffer_text_len(&self) -> usize {
        self.buffer.iter().map(event_text_len).sum()
    }

    fn pending_text_len(&self) -> usize {
        self.pending.iter().map(event_text_len).sum()
    }

    fn attempt_merge(&mut self, min_overlap: usize) -> Vec<Event> {
        if self.pending_text_len() < self.min_overlap {
            return vec![];
        }

        // 1. Reconstruct the tail text from self.buffer
        let (old_text, _) = reconstruct_text(&self.buffer);

        // 2. Reconstruct the head text from self.pending
        let (new_text, new_indices) = reconstruct_text(&self.pending);

        // 3. Find overlap
        // We look for overlaps >= min_overlap
        let overlap = find_merge_point(&old_text, &new_text, self.max_overlap, min_overlap);

        if overlap >= min_overlap && overlap > 0 {
            debug!(overlap, "EventChain: Found merge point.");

            // 4. Modify self.pending to remove the overlapping prefix.
            self.trim_pending_overlap(overlap, &new_indices);

            // 5. Success! Flush old buffer, then flush modified pending, switch to Normal.
            let mut out = Vec::new();
            out.extend(self.buffer.drain(..));
            out.extend(self.pending.drain(..));

            self.state = ChainState::Normal;
            out
        } else {
            // No overlap found yet.
            vec![]
        }
    }

    /// Remove `chars_to_skip` characters from the start of the `pending` buffer.
    fn trim_pending_overlap(&mut self, mut chars_to_skip: usize, indices: &[(usize, usize)]) {
        let mut last_consumed_deque_index = None;
        let mut partial_trim_info = None; // (deque_index, chars_to_trim)

        for &(idx, len) in indices {
            if chars_to_skip == 0 {
                break;
            }

            if len <= chars_to_skip {
                // This event is fully part of the overlap.
                chars_to_skip -= len;
                last_consumed_deque_index = Some(idx);
            } else {
                // This event needs partial trimming.
                partial_trim_info = Some((idx, chars_to_skip));
                chars_to_skip = 0;
            }
        }

        // Apply removals

        let drain_up_to = if let Some((idx, _)) = partial_trim_info {
            // We need to keep the event at `idx`, but remove everything before it.
            idx
        } else if let Some(idx) = last_consumed_deque_index {
            // We consumed this event fully. Remove it and everything before.
            // idx is inclusive, so we remove idx + 1 elements.
            idx + 1
        } else {
            0
        };

        self.pending.drain(0..drain_up_to);

        // Apply partial trim if needed.
        if let Some((_, trim_count)) = partial_trim_info
            && let Some(event) = self.pending.front_mut()
        {
            trim_event_start(event, trim_count);
        }
    }
}

/// Helper to get text length of an event (if it's a message/reasoning part).
fn event_text_len(event: &Event) -> usize {
    event
        .as_conversation_event()
        .and_then(ConversationEvent::as_chat_response)
        .map_or(0, |v| v.content().len())
}

/// Reconstruct text from a deque of events.
///
/// Returns the concatenated string and a list of (`index_in_deque`, `char_len`)
/// for mapping string positions back to events.
fn reconstruct_text(events: &VecDeque<Event>) -> (String, Vec<(usize, usize)>) {
    let mut s = String::new();
    let mut map = Vec::new();

    for (i, event) in events.iter().enumerate() {
        if let Some(content) = event
            .as_conversation_event()
            .and_then(ConversationEvent::as_chat_response)
            .map(ChatResponse::content)
            && !content.is_empty()
        {
            s.push_str(content);
            map.push((i, content.len()));
        }
    }
    (s, map)
}

/// Mutates the event to remove `count` bytes/chars from the start of its text
/// content.
fn trim_event_start(event: &mut Event, count: usize) {
    let Some(content) = event
        .as_conversation_event_mut()
        .and_then(ConversationEvent::as_chat_response_mut)
        .map(ChatResponse::content_mut)
    else {
        return;
    };

    if count < content.len() {
        content.replace_range(..count, "");
    } else {
        content.clear();
    }
}

/// Finds the merge point between two text chunks by detecting overlapping content.
///
/// Returns the number of bytes to skip from the start of `right` to merge it
/// seamlessly with `left`.
fn find_merge_point(left: &str, right: &str, max_search: usize, min_overlap: usize) -> usize {
    let max_overlap = left.len().min(right.len()).min(max_search);

    // Try progressively smaller overlaps, but stop at minimum threshold
    for overlap in (min_overlap..=max_overlap).rev() {
        let left_start = left.len() - overlap;

        // Only attempt comparison if both positions are valid UTF-8 char boundaries
        if left.is_char_boundary(left_start) && right.is_char_boundary(overlap) {
            let left_suffix = &left[left_start..];
            let right_prefix = &right[..overlap];

            if left_suffix == right_prefix {
                return overlap;
            }
        }
    }

    // No overlap found
    0
}

// TODO
// #[cfg(test)]
// mod tests {
//     use jp_conversation::{
//         event::{ChatResponse, EventKind},
//         thread::Thread,
//     };
//     use serde::{Deserialize, Serialize};
//     use test_log::test;
//     use time::macros::utc_datetime;
//
//     use super::*;
//
//     #[derive(Debug, Serialize, Deserialize, PartialEq)]
//     struct TestEvent {
//         // #[serde(with = "jp_serde::repr::base64_string")]
//         content: String,
//     }
//
//     #[test]
//     fn test_event_chain() {
//         let mut chain = EventChain::new();
//
//         // Normal mode
//         let mut events = vec![
//             Event::Part {
//                 event: ConversationEvent {
//                     timestamp: utc_datetime!(2022-01-01 00:00:00),
//                     kind: ChatResponse::Message {
//                         message: "Hello".to_string(),
//                     }
//                     .into(),
//                     metadata: Default::default(),
//                 },
//                 index: 0,
//             },
//             Event::Part {
//                 event: ConversationEvent {
//                     timestamp: utc_datetime!(2022-01-01 00:00:00),
//                     kind: ChatResponse::Message {
//                         message: "World".to_string(),
//                     }
//                     .into(),
//                     metadata: Default::default(),
//                 },
//                 index: 1,
//             },
//             Event::Finished(FinishReason::MaxTokens),
//         ];
//
//         let mut out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![]);
//
//         out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![]);
//
//         out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![]);
//
//         out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![Event::Part {
//             event: ConversationEvent {
//                 timestamp: utc_datetime!(2022-01-01 00:00:00),
//                 kind: ChatResponse::Message {
//                     message: "Hello".to_string(),
//                 }
//                 .into(),
//                 metadata: Default::default(),
//             },
//             index: 0,
//         }]);
//
//         out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![Event::Part {
//             event: ConversationEvent {
//                 timestamp: utc_datetime!(2022-01-01 00:00:00),
//                 kind: EventKind::ChatResponse(ChatResponse::Message {
//                     message: "World".to_string(),
//                 }),
//                 metadata: Default::default(),
//             },
//             index: 0,
//         }]);
//         assert_eq!(chain.ingest(events.remove(0)), vec![]);
//
//         // Merging mode
//         chain.state = ChainState::Merging;
//         chain.min_overlap = 3;
//
//         out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![Event::Part {
//             event: ConversationEvent {
//                 timestamp: utc_datetime!(2022-01-01 00:00:00),
//                 kind: EventKind::ChatResponse(ChatResponse::Message {
//                     message: "Hello".to_string(),
//                 }),
//                 metadata: Default::default(),
//             },
//             index: 0,
//         }]);
//
//         out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![Event::Part {
//             event: ConversationEvent {
//                 timestamp: utc_datetime!(2022-01-01 00:00:00),
//                 kind: EventKind::ChatResponse(ChatResponse::Message {
//                     message: "World".to_string(),
//                 }),
//                 metadata: Default::default(),
//             },
//             index: 0,
//         }]);
//         assert_eq!(chain.ingest(events.remove(0)), vec![]);
//
//         // Merging mode, with overlap
//         chain.state = ChainState::Merging;
//         chain.min_overlap = 4;
//
//         out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![Event::Part {
//             event: ConversationEvent {
//                 timestamp: utc_datetime!(2022-01-01 00:00:00),
//                 kind: EventKind::ChatResponse(ChatResponse::Message {
//                     message: "Hello".to_string(),
//                 }),
//                 metadata: Default::default(),
//             },
//             index: 0,
//         }]);
//
//         out = chain.ingest(events.remove(0));
//         assert_eq!(out, vec![Event::Part {
//             event: ConversationEvent {
//                 timestamp: utc_datetime!(2022-01-01 00:00:00),
//                 kind: EventKind::ChatResponse(ChatResponse::Message {
//                     message: "World".to_string(),
//                 }),
//                 metadata: Default::default(),
//             },
//             index: 0,
//         }]);
//         assert_eq!(chain.ingest(events.remove(0)), vec![]);
//     }
// }
