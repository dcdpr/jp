use chrono::{TimeZone as _, Utc};

use crate::{
    ConversationEvent, ConversationStream, EventKind,
    event::{ChatRequest, ChatResponse, TurnStart},
};

fn ts(h: u32, m: u32, s: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2020, 1, 1, h, m, s).unwrap()
}

#[test]
fn empty_stream_yields_no_turns() {
    let stream = ConversationStream::new_test();
    assert_eq!(stream.iter_turns().len(), 0);
}

#[test]
fn single_turn() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Hello"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("Hi.\n\n"), ts(0, 0, 2)),
    ]);

    let turns: Vec<_> = stream.iter_turns().collect();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].iter().count(), 3);
}

#[test]
fn turn_index() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q2"), ts(0, 1, 1)),
        ConversationEvent::new(TurnStart, ts(0, 2, 0)),
        ConversationEvent::new(ChatRequest::from("Q3"), ts(0, 2, 1)),
    ]);

    let turns: Vec<_> = stream.iter_turns().collect();
    assert_eq!(turns[0].index(), 0);
    assert_eq!(turns[1].index(), 1);
    assert_eq!(turns[2].index(), 2);
}

#[test]
fn turn_index_with_implicit_leading_turn() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(ChatRequest::from("orphan"), ts(0, 0, 0)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 1, 1)),
    ]);

    let turns: Vec<_> = stream.iter_turns().collect();
    assert_eq!(turns[0].index(), 0); // implicit turn
    assert_eq!(turns[1].index(), 1);
}

#[test]
fn multiple_turns() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("A1.\n\n"), ts(0, 0, 2)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q2"), ts(0, 1, 1)),
        ConversationEvent::new(ChatResponse::message("A2.\n\n"), ts(0, 1, 2)),
        ConversationEvent::new(TurnStart, ts(0, 2, 0)),
        ConversationEvent::new(ChatRequest::from("Q3"), ts(0, 2, 1)),
    ]);

    let turns: Vec<_> = stream.iter_turns().collect();
    assert_eq!(turns.len(), 3);

    // First turn: TurnStart + ChatRequest + ChatResponse
    assert_eq!(turns[0].iter().count(), 3);
    // Second turn: TurnStart + ChatRequest + ChatResponse
    assert_eq!(turns[1].iter().count(), 3);
    // Third turn: TurnStart + ChatRequest
    assert_eq!(turns[2].iter().count(), 2);
}

#[test]
fn events_before_first_turn_start_form_implicit_turn() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(ChatRequest::from("orphan"), ts(0, 0, 0)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 1, 1)),
    ]);

    let turns: Vec<_> = stream.iter_turns().collect();
    assert_eq!(turns.len(), 2);
    // Implicit turn has the orphan ChatRequest
    assert!(matches!(
        turns[0].iter().next().unwrap().event.kind,
        EventKind::ChatRequest(_)
    ));
}

#[test]
fn double_ended_iteration() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("first"), ts(0, 0, 1)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("second"), ts(0, 1, 1)),
    ]);

    let mut iter = stream.iter_turns();
    let last = iter.next_back().unwrap();

    // The last turn should contain "second"
    let req = last.iter().find_map(|e| e.event.as_chat_request()).unwrap();
    assert_eq!(req.content, "second");
}

#[test]
fn exact_size() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q2"), ts(0, 1, 1)),
        ConversationEvent::new(TurnStart, ts(0, 2, 0)),
        ConversationEvent::new(ChatRequest::from("Q3"), ts(0, 2, 1)),
    ]);

    assert_eq!(stream.iter_turns().len(), 3);
}

#[test]
fn retain_last_turns_keeps_last_n() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
        ConversationEvent::new(ChatResponse::message("A1.\n\n"), ts(0, 0, 2)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q2"), ts(0, 1, 1)),
        ConversationEvent::new(ChatResponse::message("A2.\n\n"), ts(0, 1, 2)),
        ConversationEvent::new(TurnStart, ts(0, 2, 0)),
        ConversationEvent::new(ChatRequest::from("Q3"), ts(0, 2, 1)),
    ]);

    stream.retain_last_turns(1);

    let turns: Vec<_> = stream.iter_turns().collect();
    assert_eq!(turns.len(), 1);
    let req = turns[0]
        .iter()
        .find_map(|e| e.event.as_chat_request())
        .unwrap();
    assert_eq!(req.content, "Q3");
}

#[test]
fn retain_last_turns_noop_when_fewer_turns() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
    ]);

    stream.retain_last_turns(5);

    assert_eq!(stream.iter_turns().len(), 1);
}

#[test]
fn retain_last_turns_zero_clears() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
    ]);

    stream.retain_last_turns(0);

    assert_eq!(stream.iter_turns().len(), 0);
}
