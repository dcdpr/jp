//! What a reader should show for a conversation.
//!
//! Two judgements live here, and both are about the conversation model rather
//! than about any one reader.
//! The first is which events have prose to show: a `chat_request` is a message
//! from the user, a `chat_response` carrying a `message` is one from the
//! assistant, and nothing else draws.
//! The second is where the turn boundaries fall, which is not a rule a reader
//! can recover from the event shape — events before the first `TurnStart` form
//! an implicit leading turn, and a `TurnStart` opens a new turn only when the
//! one before it holds something.
//!
//! Both belong on this side of the boundary, where the model lives, rather than
//! being re-derived by every reader.
//!
//! Scoped to this crate for now.
//! The terminal renderer and the web view make the same judgement in their own
//! code, and a projection shared by all three is a larger change than the app
//! needs today.

use jp_conversation::{
    ConversationEvent, EventKind, event::ChatResponse, rfc3339, stream::ConversationStream,
};
use serde::Serialize;

/// One turn, as a reader should present it.
///
/// A turn with nothing to show is absent rather than empty, so a reader can
/// draw a boundary between every pair of turns it receives without checking
/// whether either holds anything.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DisplayTurn {
    /// Where the turn sits in the conversation, counting from zero.
    ///
    /// The position among *all* turns, so the numbering skips any that had
    /// nothing to show.
    /// That keeps an index pointing at the same turn whatever a later build
    /// decides to draw.
    pub index: usize,

    /// What the turn has to show, oldest first.
    pub events: Vec<DisplayEvent>,
}

/// One event, as a reader should present it.
///
/// The `type` tag names the presentation, not the stored event kind: a caller
/// switches on it to decide how to draw, and needs no table of event kinds of
/// its own.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum DisplayEvent {
    /// A message the user sent.
    UserMessage {
        timestamp: String,

        /// Who wrote it, when a display name was configured at the time.
        #[serde(skip_serializing_if = "Option::is_none")]
        author: Option<String>,

        text: String,
    },

    /// A message the assistant replied with.
    AssistantMessage { timestamp: String, text: String },
}

/// Project a conversation onto the turns a reader shows.
///
/// Reads the typed stream rather than a serialized copy of it.
/// Serializing to get here would base64-encode the fields storage encodes and
/// then decode them again, reparse and reformat every timestamp, and allocate a
/// whole second copy of the conversation — all to read four fields off it.
pub(crate) fn project_turns(stream: &ConversationStream) -> Vec<DisplayTurn> {
    let mut turns: Vec<DisplayTurn> = Vec::new();

    // `iter_events_by_turn` rather than `iter_turns`: the latter resolves and
    // clones the accumulated config for every event and materializes the whole
    // stream up front, and none of that is read here.
    for (index, event) in stream.iter_events_by_turn() {
        let Some(event) = project_event(event) else {
            continue;
        };

        match turns.last_mut() {
            Some(turn) if turn.index == index => turn.events.push(event),
            _ => turns.push(DisplayTurn {
                index,
                events: vec![event],
            }),
        }
    }

    turns
}

/// One event, or `None` when it has no prose to show.
///
/// The timestamp is formatted inside each arm rather than up front, because
/// most events in a long conversation are tool calls and reasoning and never
/// reach a caller.
fn project_event(event: &ConversationEvent) -> Option<DisplayEvent> {
    match &event.kind {
        // Content is not optional on a request, so unlike the response below
        // there is no empty case to fall through to.
        EventKind::ChatRequest(request) => Some(DisplayEvent::UserMessage {
            timestamp: rfc3339(event.timestamp),
            author: request.author.clone(),
            text: request.content.clone(),
        }),

        // A response carrying reasoning or structured data has no message, and
        // showing either is out of scope for the reader.
        EventKind::ChatResponse(ChatResponse::Message { message }) => {
            Some(DisplayEvent::AssistantMessage {
                timestamp: rfc3339(event.timestamp),
                text: message.clone(),
            })
        }

        _ => None,
    }
}

#[cfg(test)]
#[path = "display_tests.rs"]
mod tests;
