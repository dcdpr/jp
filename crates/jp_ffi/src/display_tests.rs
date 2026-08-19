use chrono::{DateTime, TimeDelta, Utc};
use datetime_literal::datetime;
use jp_conversation::{
    ConversationEvent,
    event::{ChatRequest, ChatResponse, ToolCallRequest, TurnStart},
    stream::ConversationStream,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

/// One fixed moment, so every expected timestamp below is the same string.
fn at() -> DateTime<Utc> {
    datetime!(2024-09-01 10:00:00 Z)
}

fn event(kind: impl Into<EventKind>) -> ConversationEvent {
    ConversationEvent::new(kind, at())
}

/// A stream holding `kinds`, all at the same moment.
///
/// Built through `from_parts` because the stream's own mutators timestamp with
/// the wall clock, and every assertion below names the timestamp it expects.
fn stream(kinds: Vec<EventKind>) -> ConversationStream {
    events(kinds.into_iter().map(event).collect())
}

/// A stream holding `events`.
fn events(events: Vec<ConversationEvent>) -> ConversationStream {
    // The config a stream is built on is required and irrelevant here, so it
    // comes from the crate's own test stream rather than being spelled out.
    let (config, _) = ConversationStream::new_test().to_parts().unwrap();
    let events = events
        .into_iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();

    ConversationStream::from_parts(config, events).unwrap()
}

#[test]
fn projects_a_chat_request_as_a_user_message() {
    let stream = stream(vec![
        TurnStart.into(),
        ChatRequest {
            content: "What does this do?".to_owned(),
            schema: None,
            author: Some("Jean".to_owned()),
        }
        .into(),
    ]);

    assert_eq!(project_turns(&stream), vec![DisplayTurn {
        index: 0,
        events: vec![DisplayEvent::UserMessage {
            timestamp: "2024-09-01T10:00:00Z".to_owned(),
            author: Some("Jean".to_owned()),
            text: "What does this do?".to_owned(),
        }],
    }]);
}

/// A request authored before a display name was configured has no author.
#[test]
fn projects_a_chat_request_without_an_author() {
    let stream = stream(vec![TurnStart.into(), ChatRequest::from("hi").into()]);

    assert_eq!(project_turns(&stream), vec![DisplayTurn {
        index: 0,
        events: vec![DisplayEvent::UserMessage {
            timestamp: "2024-09-01T10:00:00Z".to_owned(),
            author: None,
            text: "hi".to_owned(),
        }],
    }]);
}

#[test]
fn projects_a_chat_response_as_an_assistant_message() {
    let stream = stream(vec![
        TurnStart.into(),
        ChatRequest::from("hi").into(),
        ChatResponse::message("It reads conversations.").into(),
    ]);

    assert_eq!(project_turns(&stream), vec![DisplayTurn {
        index: 0,
        events: vec![
            DisplayEvent::UserMessage {
                timestamp: "2024-09-01T10:00:00Z".to_owned(),
                author: None,
                text: "hi".to_owned(),
            },
            DisplayEvent::AssistantMessage {
                timestamp: "2024-09-01T10:00:00Z".to_owned(),
                text: "It reads conversations.".to_owned(),
            },
        ],
    }]);
}

/// Every event kind that is not a message is absent from the projection —
/// reasoning and structured output included, both of which are chat responses
/// carrying no message and must not be mistaken for the assistant's reply.
#[test]
fn drops_every_event_with_no_prose_to_show() {
    let stream = stream(vec![
        TurnStart.into(),
        ChatRequest::from("hi").into(),
        ChatResponse::reasoning("thinking").into(),
        ToolCallRequest::new(
            "call-1".to_owned(),
            "read_file".to_owned(),
            serde_json::Map::new(),
        )
        .into(),
        ChatResponse::structured(json!({ "answer": 42 })).into(),
        ChatResponse::message("done").into(),
    ]);

    assert_eq!(project_turns(&stream), vec![DisplayTurn {
        index: 0,
        events: vec![
            DisplayEvent::UserMessage {
                timestamp: "2024-09-01T10:00:00Z".to_owned(),
                author: None,
                text: "hi".to_owned(),
            },
            DisplayEvent::AssistantMessage {
                timestamp: "2024-09-01T10:00:00Z".to_owned(),
                text: "done".to_owned(),
            },
        ],
    }]);
}

/// The boundary rule is the stream's own: a `TurnStart` opens a new turn only
/// when the one before it holds something, so the leading marker here does not
/// produce an empty turn 0 ahead of the first request.
#[test]
fn groups_events_into_the_turn_they_belong_to() {
    let stream = stream(vec![
        TurnStart.into(),
        ChatRequest::from("first question").into(),
        ChatResponse::message("first answer").into(),
        TurnStart.into(),
        ChatRequest::from("second question").into(),
        ChatResponse::message("second answer").into(),
    ]);

    let turns = project_turns(&stream);
    let texts: Vec<(usize, Vec<&str>)> = turns
        .iter()
        .map(|turn| {
            let texts = turn
                .events
                .iter()
                .map(|event| match event {
                    DisplayEvent::UserMessage { text, .. }
                    | DisplayEvent::AssistantMessage { text, .. } => text.as_str(),
                })
                .collect();

            (turn.index, texts)
        })
        .collect();

    assert_eq!(texts, vec![
        (0, vec!["first question", "first answer"]),
        (1, vec!["second question", "second answer"]),
    ]);
}

/// A turn whose every event is a tool call is absent rather than empty, so a
/// reader drawing a boundary between consecutive turns never draws two against
/// nothing.
///
/// The index of the turn after it still counts the dropped one, so an index
/// names the same turn whatever a later build decides to draw.
#[test]
fn drops_a_turn_with_nothing_to_show_and_keeps_the_numbering() {
    let stream = stream(vec![
        TurnStart.into(),
        ChatRequest::from("visible").into(),
        TurnStart.into(),
        ToolCallRequest::new(
            "call-1".to_owned(),
            "read_file".to_owned(),
            serde_json::Map::new(),
        )
        .into(),
        TurnStart.into(),
        ChatRequest::from("also visible").into(),
    ]);

    let indices: Vec<usize> = project_turns(&stream)
        .iter()
        .map(|turn| turn.index)
        .collect();

    assert_eq!(indices, vec![0, 2]);
}

/// Events written before any `TurnStart` are a turn of their own, which is the
/// stream's implicit leading turn rather than something invented here.
#[test]
fn projects_events_before_the_first_turn_start_as_the_leading_turn() {
    let stream = stream(vec![
        ChatRequest::from("no marker ahead of me").into(),
        TurnStart.into(),
        ChatRequest::from("after the marker").into(),
    ]);

    let indices: Vec<usize> = project_turns(&stream)
        .iter()
        .map(|turn| turn.index)
        .collect();

    assert_eq!(indices, vec![0, 1]);
}

#[test]
fn projects_an_empty_stream_as_no_turns() {
    assert_eq!(project_turns(&events(vec![])), vec![]);
}

/// Sub-second precision survives, because a reader ordering events needs it and
/// two events in one millisecond is ordinary.
#[test]
fn keeps_sub_second_precision_in_a_timestamp() {
    let stream = events(vec![ConversationEvent::new(
        ChatRequest::from("hi"),
        at() + TimeDelta::microseconds(418_293),
    )]);

    assert_eq!(project_turns(&stream), vec![DisplayTurn {
        index: 0,
        events: vec![DisplayEvent::UserMessage {
            timestamp: "2024-09-01T10:00:00.418293Z".to_owned(),
            author: None,
            text: "hi".to_owned(),
        }],
    }]);
}

/// The wire shape a reader decodes: turns carrying an index and their events,
/// each tagged with its presentation.
#[test]
fn serializes_turns_carrying_events_tagged_by_presentation() {
    let stream = stream(vec![
        TurnStart.into(),
        ChatRequest::from("hi").into(),
        ChatResponse::message("hello").into(),
    ]);

    assert_eq!(
        serde_json::to_string(&project_turns(&stream)).unwrap(),
        r#"[{"index":0,"events":[{"type":"user_message","timestamp":"2024-09-01T10:00:00Z","text":"hi"},{"type":"assistant_message","timestamp":"2024-09-01T10:00:00Z","text":"hello"}]}]"#
    );
}
