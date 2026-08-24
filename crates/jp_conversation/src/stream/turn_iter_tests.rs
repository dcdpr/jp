use chrono::{TimeZone as _, Utc};
use jp_config::PartialAppConfig;

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

/// Every event's turn index according to `iter_turns`, which resolves per-event
/// configuration, paired with the same according to the allocation-free
/// `iter_events_by_turn`.
///
/// The two must agree exactly: callers pick between them on cost alone.
fn turn_indices(stream: &ConversationStream) -> (Vec<usize>, Vec<usize>) {
    let via_turns = stream
        .iter_turns()
        .flat_map(|turn| turn.iter().map(|_| turn.index()).collect::<Vec<_>>())
        .collect();
    let direct = stream.iter_events_by_turn().map(|(turn, _)| turn).collect();

    (via_turns, direct)
}

#[test]
fn events_by_turn_agrees_with_iter_turns() {
    // Leading `TurnStart`, an interleaved config delta, and a trailing turn:
    // the delta must not shift any turn number, since `iter_turns` never sees it.
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
    ]);
    stream.add_config_delta(PartialAppConfig::default());
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q2"), ts(0, 1, 1)),
    ]);

    let (via_turns, direct) = turn_indices(&stream);
    assert_eq!(via_turns, direct);
    assert_eq!(direct, vec![0, 0, 1, 1]);
    assert_eq!(stream.turn_count(), 2);
}

#[test]
fn events_by_turn_agrees_on_an_implicit_leading_turn() {
    // No leading `TurnStart`: the orphan events form turn 0 and the explicit
    // marker opens turn 1.
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(ChatRequest::from("orphan"), ts(0, 0, 1)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 1, 1)),
    ]);

    let (via_turns, direct) = turn_indices(&stream);
    assert_eq!(via_turns, direct);
    assert_eq!(direct, vec![0, 1, 1]);
    assert_eq!(stream.turn_count(), 2);
}

#[test]
fn events_by_turn_is_empty_for_an_empty_stream() {
    let stream = ConversationStream::new_test();
    assert_eq!(stream.iter_events_by_turn().count(), 0);
    assert_eq!(stream.turn_count(), 0);
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

#[test]
fn retain_turns_keeps_a_leading_window() {
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

    stream.retain_turns(|index| index == 0);

    let turns: Vec<_> = stream.iter_turns().collect();
    assert_eq!(turns.len(), 1);
    let req = turns[0]
        .iter()
        .find_map(|e| e.event.as_chat_request())
        .unwrap();
    assert_eq!(req.content, "Q1");
}

#[test]
fn retain_turns_keeps_two_windows() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q2"), ts(0, 1, 1)),
        ConversationEvent::new(TurnStart, ts(0, 2, 0)),
        ConversationEvent::new(ChatRequest::from("Q3"), ts(0, 2, 1)),
        ConversationEvent::new(TurnStart, ts(0, 3, 0)),
        ConversationEvent::new(ChatRequest::from("Q4"), ts(0, 3, 1)),
    ]);

    // Keep the first turn and the last two, dropping turn 2.
    stream.retain_turns(|index| index == 0 || index >= 2);

    let requests: Vec<_> = stream
        .iter()
        .filter_map(|e| e.event.as_chat_request())
        .map(|r| r.content.clone())
        .collect();
    assert_eq!(requests, vec!["Q1", "Q3", "Q4"]);
}

#[test]
fn retain_turns_noop_when_every_turn_is_kept() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
    ]);

    stream.retain_turns(|index| index < 5);

    assert_eq!(stream.iter_turns().len(), 1);
}

#[test]
fn retain_turns_numbers_an_implicit_leading_turn_like_iter_turns() {
    // `[A, TurnStart, B]` is turn 0 = `[A]` and turn 1 = `[TurnStart, B]`:
    // a `TurnStart` opens a turn only when the current one already holds an
    // event. A predicate built from resolved turn positions must agree, or
    // `--first 1` on a fork keeps B instead of A.
    let events = || {
        vec![
            ConversationEvent::new(ChatRequest::from("A"), ts(0, 0, 0)),
            ConversationEvent::new(TurnStart, ts(0, 1, 0)),
            ConversationEvent::new(ChatRequest::from("B"), ts(0, 1, 1)),
        ]
    };

    let mut stream = ConversationStream::new_test();
    stream.extend(events());
    assert_eq!(
        stream.iter_turns().len(),
        2,
        "fixture has an implicit turn 0"
    );

    stream.retain_turns(|index| index == 0);
    let requests: Vec<_> = stream
        .iter()
        .filter_map(|e| e.event.as_chat_request())
        .map(|r| r.content.clone())
        .collect();
    assert_eq!(requests, vec!["A"], "turn 0 is the unmarked prefix");

    let mut stream = ConversationStream::new_test();
    stream.extend(events());

    stream.retain_turns(|index| index == 1);
    let requests: Vec<_> = stream
        .iter()
        .filter_map(|e| e.event.as_chat_request())
        .map(|r| r.content.clone())
        .collect();
    assert_eq!(requests, vec!["B"], "turn 1 is the marked suffix");
}

#[test]
fn retain_turns_keeping_nothing_clears() {
    let mut stream = ConversationStream::new_test();
    stream.extend(vec![
        ConversationEvent::new(TurnStart, ts(0, 0, 0)),
        ConversationEvent::new(ChatRequest::from("Q1"), ts(0, 0, 1)),
        ConversationEvent::new(TurnStart, ts(0, 1, 0)),
        ConversationEvent::new(ChatRequest::from("Q2"), ts(0, 1, 1)),
    ]);

    stream.retain_turns(|_| false);

    assert_eq!(stream.iter_turns().len(), 0);
}
