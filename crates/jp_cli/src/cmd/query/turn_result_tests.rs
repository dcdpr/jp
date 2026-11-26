use jp_conversation::event::TurnStart;

use super::*;
use crate::cmd::query::PendingStreamTrim;

/// Append a turn whose structured response is `data`.
fn add_structured_turn(stream: &mut ConversationStream, data: Value) {
    stream.start_turn("question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::structured(data))
        .build()
        .unwrap();
}

/// A stream of one turn whose structured response is `data`.
fn stream_with_structured(data: Value) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    add_structured_turn(&mut stream, data);
    stream
}

/// A schema whose root `type` is `kind`.
fn schema_of_type(kind: &str) -> Map<String, Value> {
    Map::from_iter([("type".to_owned(), Value::from(kind))])
}

/// A string is what a string schema asks for, so it is an answer and not a sign
/// the response was cut short.
#[test]
fn turn_structured_data_keeps_a_string_a_string_schema_asks_for() {
    let stream = stream_with_structured(Value::from("a summary"));

    assert_eq!(
        turn_structured_data(&stream, None, Some(&schema_of_type("string"))),
        Ok(Value::from("a summary"))
    );
}

#[test]
fn turn_structured_data_refuses_a_string_an_object_schema_rules_out() {
    let stream = stream_with_structured(Value::from(r#"{"summary": "sho"#));

    assert_eq!(
        turn_structured_data(&stream, None, Some(&schema_of_type("object"))),
        Err(StructuredDataError::NotJson)
    );
}

/// `jp query --schema` stopped before its turn started has no answer of its
/// own, whatever the previous turn left behind.
#[test]
fn turn_structured_data_refuses_a_turn_that_did_not_start() {
    let stream = stream_with_structured(serde_json::json!({"turn": 1}));
    let before = last_request_id(&stream);

    assert_eq!(
        turn_structured_data(&stream, before.as_ref(), Some(&schema_of_type("object"))),
        Err(StructuredDataError::NotStarted)
    );
}

/// Sanitizing the stream before the turn drops a trailing `TurnStart` that has
/// nothing after it, which moves the last turn marker back to an earlier turn
/// although this run appended nothing.
#[test]
fn turn_structured_data_refuses_a_turn_that_did_not_start_after_sanitizing() {
    let mut stream = stream_with_structured(serde_json::json!({"turn": 1}));
    stream.push_event(ConversationEvent::now(TurnStart::default()));
    let before = last_request_id(&stream);

    stream.sanitize();

    assert_eq!(
        turn_structured_data(&stream, before.as_ref(), Some(&schema_of_type("object"))),
        Err(StructuredDataError::NotStarted)
    );
}

/// `--replay` swaps the last turn for a new one, so the conversation has as
/// many turns as before, and the new turn's answer is still this run's.
#[test]
fn turn_structured_data_takes_a_replayed_turns_answer() {
    let mut stream = stream_with_structured(serde_json::json!({"turn": 1}));
    let before = last_request_id(&stream);

    PendingStreamTrim {
        replay_turn: true,
        pop_request: false,
    }
    .apply(&mut stream);
    add_structured_turn(&mut stream, serde_json::json!({"turn": 2}));

    assert_eq!(
        turn_structured_data(&stream, before.as_ref(), Some(&schema_of_type("object"))),
        Ok(serde_json::json!({"turn": 2}))
    );
}
