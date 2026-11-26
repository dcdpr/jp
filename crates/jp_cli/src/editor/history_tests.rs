use chrono::{DateTime, FixedOffset, TimeZone as _, Utc};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse, TurnStart},
};
use serde_json::Map;

use crate::editor::build_history_text;

fn at(secs: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, secs).unwrap()
}

/// Keep comrak's default escaping (`foo\_bar`), because
/// `experimental_minimize_commonmark` re-parses the message once per escape.
#[test]
fn history_keeps_default_escapes() {
    let mut stream = ConversationStream::new_test();
    stream.push_event(ConversationEvent::new(
        ChatRequest::from("Rename foo_bar."),
        at(0),
    ));

    let text = build_history_text(&stream, FixedOffset::east_opt(0).unwrap());

    assert_eq!(
        text,
        "\n# Conversation History (last 10 entries)\n\n## You on 2020-01-01 00:00:00\n\nRename \
         foo\\_bar.\n\n"
    );
}

/// Render chat requests and responses newest first, omitting reasoning and tool
/// calls before applying the 10-entry limit so the first turn's 10 tool events
/// do not push its request out of the history.
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
         2020-01-01 00:00:09\n\nSecond answer\n\n\n\n## You on 2020-01-01 00:00:07\n\nSecond \
         question\n\n\n\n## Assistant (anthropic/test) on 2020-01-01 00:00:05\n\nFirst \
         answer\n\n\n\n## You on 2020-01-01 00:00:01\n\nFirst question\n\n"
    );
}
