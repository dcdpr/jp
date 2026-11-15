use chrono::{DateTime, FixedOffset, TimeZone as _, Utc};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse, TurnStart},
};
use serde_json::Map;

use super::build_history_text;

fn at(secs: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, secs).unwrap()
}

/// The history shows the chat requests and responses of the last turns, newest
/// first.
/// Reasoning and tool calls are omitted, and do not count toward the ten-entry
/// limit: the first turn's ten tool call events would otherwise push its
/// request out of the history.
#[test]
fn history_omits_reasoning_and_tool_calls() {
    let mut stream = ConversationStream::new_test();
    stream.push_event(ConversationEvent::new(TurnStart::default(), at(0)));
    stream.push_event(ConversationEvent::new(
        ChatRequest::from("First question"),
        at(1),
    ));
    stream.push_event(ConversationEvent::new(
        ChatResponse::reasoning("Let me look at the files."),
        at(2),
    ));
    for idx in 0..5 {
        stream.push_event(ConversationEvent::new(
            ToolCallRequest {
                id: format!("tc{idx}"),
                name: "read_file".into(),
                arguments: Map::new(),
            },
            at(3),
        ));
        stream.push_event(ConversationEvent::new(
            ToolCallResponse {
                id: format!("tc{idx}"),
                result: Ok("fn main() {}".into()),
            },
            at(4),
        ));
    }
    stream.push_event(ConversationEvent::new(
        ChatResponse::message("First answer"),
        at(5),
    ));
    stream.push_event(ConversationEvent::new(TurnStart::default(), at(6)));
    stream.push_event(ConversationEvent::new(
        ChatRequest::from("Second question"),
        at(7),
    ));
    stream.push_event(ConversationEvent::new(
        ChatResponse::reasoning("Easy one."),
        at(8),
    ));
    stream.push_event(ConversationEvent::new(
        ChatResponse::message("Second answer"),
        at(9),
    ));

    let text = build_history_text(&stream, FixedOffset::east_opt(0).unwrap());

    assert_eq!(
        text,
        "\n# Conversation History (last 10 entries)\n\n## Assistant (anthropic/test) on \
         2020-01-01 00:00:09\n\nSecond answer\n\n## You on 2020-01-01 00:00:07\n\nSecond \
         question\n\n\n\n## Assistant (anthropic/test) on 2020-01-01 00:00:05\n\nFirst \
         answer\n\n## You on 2020-01-01 00:00:01\n\nFirst question\n\n"
    );
}
