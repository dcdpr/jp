use indexmap::{IndexMap, map::Entry};
use jp_conversation::{
    ConversationEvent, EventKind,
    event::{ChatResponse, ToolCallRequest},
};
use serde_json::Value;
use tracing::warn;

use crate::event::Event;

/// A buffering state machine that consumes multiplexed streaming events and
/// produces coalesced events keyed by index.
pub struct EventAggregator {
    /// The currently accumulating events, keyed by stream index.
    pending: IndexMap<usize, ConversationEvent>,
}

impl EventAggregator {
    /// Create a new, empty aggregator.
    pub fn new() -> Self {
        Self {
            pending: IndexMap::new(),
        }
    }

    /// Consumes a single streaming [`Event`] and returns a vector of zero or
    /// more "completed" `Event`s.
    ///
    /// For consistency, we always send a `Flush` event after flushing a merged
    /// `Part` event. While this is not strictly necessary, it makes the API
    /// more consistent to use, regardless of whether the event aggregator is
    /// used or not.
    pub fn ingest(&mut self, event: Event) -> Vec<Event> {
        match event {
            Event::Part { index, event } => match self.pending.entry(index) {
                // Nothing buffered for this index, start buffering.
                Entry::Vacant(e) => {
                    e.insert(event);
                    vec![]
                }
                Entry::Occupied(mut e) => match try_merge_events(e.get_mut(), event) {
                    // Merge succeeded. Continue buffering.
                    Ok(()) => vec![],
                    // Merge failed (types were different). Force flush the OLD
                    // event, replace it with the NEW event.
                    Err(unmerged) => vec![
                        Event::Part {
                            index,
                            event: e.insert(unmerged),
                        },
                        Event::flush(index),
                    ],
                },
            },

            Event::Flush { index, metadata } => {
                if let Some(event) = self.pending.shift_remove(&index) {
                    vec![
                        Event::Part {
                            index,
                            event: event.with_metadata(metadata),
                        },
                        Event::flush(index),
                    ]
                } else {
                    if !metadata.is_empty() {
                        warn!(
                            index,
                            metadata = ?metadata,
                            "Received Flush with metadata for empty index."
                        );
                    }

                    vec![]
                }
            }

            Event::Finished(reason) => self
                .pending
                .drain(..)
                .flat_map(|(index, event)| vec![Event::Part { index, event }, Event::flush(index)])
                .chain(std::iter::once(Event::Finished(reason)))
                .collect(),
        }
    }
}

/// Attempts to merge `incoming` into `target`. Returns `Ok(())` if successful,
/// or `Err(incoming)` if the events were incompatible (e.g., different types),
/// passing ownership of the incoming event back to the caller.
fn try_merge_events(
    target: &mut ConversationEvent,
    incoming: ConversationEvent,
) -> Result<(), ConversationEvent> {
    let ConversationEvent {
        kind,
        metadata,
        timestamp,
    } = incoming;

    match (&mut target.kind, kind) {
        (EventKind::ChatResponse(t_resp), EventKind::ChatResponse(i_resp)) => {
            match merge_chat_responses(t_resp, i_resp) {
                Ok(()) => {
                    // Merge successful.
                    //
                    // Now merge the remaining fields from the destructured
                    // event.
                    target.metadata.extend(metadata);
                    target.timestamp = timestamp;

                    Ok(())
                }
                Err(returned_resp) => {
                    // Merge failed (variant mismatch).
                    //
                    // Reconstruct the event using the returned response and
                    // original fields.
                    Err(ConversationEvent {
                        kind: EventKind::ChatResponse(returned_resp),
                        metadata,
                        timestamp,
                    })
                }
            }
        }
        (EventKind::ToolCallRequest(t_tool), EventKind::ToolCallRequest(i_tool)) => {
            merge_tool_calls(t_tool, i_tool);
            target.metadata.extend(metadata);
            target.timestamp = timestamp;

            Ok(())
        }
        // Mismatch in high-level types (e.g. ChatResponse vs ToolCallRequest).
        // Reconstruct the event and return it as an error.
        (_, other_kind) => Err(ConversationEvent {
            kind: other_kind,
            metadata,
            timestamp,
        }),
    }
}

/// Merges two `ChatResponse` items.
///
/// Returns `Err(incoming)` if they are different variants (Message vs
/// Reasoning).
fn merge_chat_responses(
    target: &mut ChatResponse,
    incoming: ChatResponse,
) -> Result<(), ChatResponse> {
    match (target, incoming) {
        (ChatResponse::Message { message: t_msg }, ChatResponse::Message { message: i_msg }) => {
            t_msg.push_str(&i_msg);
            Ok(())
        }
        (
            ChatResponse::Reasoning { reasoning: t_reas },
            ChatResponse::Reasoning { reasoning: i_reas },
        ) => {
            t_reas.push_str(&i_reas);
            Ok(())
        }

        // Variants didn't match.
        (_, incoming) => Err(incoming),
    }
}

/// Merges two `ToolCallRequest` items.
fn merge_tool_calls(target: &mut ToolCallRequest, incoming: ToolCallRequest) {
    if target.id.is_empty() && !incoming.id.is_empty() {
        target.id = incoming.id;
    }

    if target.name.is_empty() && !incoming.name.is_empty() {
        target.name = incoming.name;
    }

    for (key, val) in incoming.arguments {
        match target.arguments.get_mut(&key) {
            Some(existing_val) => {
                if let (Value::String(s1), Value::String(s2)) = (existing_val, &val) {
                    s1.push_str(s2);
                } else {
                    // Overwrite non-string values
                    *target.arguments.entry(key).or_insert(Value::Null) = val;
                }
            }
            None => {
                target.arguments.insert(key, val);
            }
        }
    }
}
