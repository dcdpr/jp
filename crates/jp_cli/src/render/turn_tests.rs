use jp_conversation::{ConversationStream, event::ChatRequest};

use super::{TurnOrigin, turn_detail};

/// A single-turn stream whose turn carries a timestamp, so `turn_detail`
/// produces `Some`.
/// The "ago" suffix is time-dependent, so assertions only check the turn-number
/// prefix.
fn single_turn() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));
    stream
}

#[test]
fn kept_turn_shows_its_raw_number() {
    let stream = single_turn();
    let turn = stream.iter_turns().next().unwrap();

    let detail = turn_detail(&turn, TurnOrigin::Kept(5)).unwrap();

    assert!(detail.starts_with("turn 6, "), "got: {detail}");
}

#[test]
fn multi_turn_summary_shows_the_collapsed_range() {
    let stream = single_turn();
    let turn = stream.iter_turns().next().unwrap();

    let detail = turn_detail(&turn, TurnOrigin::Summary { from: 1, to: 4 }).unwrap();

    assert!(detail.starts_with("turns 2\u{2013}5, "), "got: {detail}");
}

#[test]
fn single_turn_summary_shows_one_number() {
    let stream = single_turn();
    let turn = stream.iter_turns().next().unwrap();

    let detail = turn_detail(&turn, TurnOrigin::Summary { from: 2, to: 2 }).unwrap();

    assert!(detail.starts_with("turn 3, "), "got: {detail}");
}
