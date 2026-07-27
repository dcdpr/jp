use jp_conversation::ConversationStream;
use jp_llm::event::FinishReason;

use super::{collect_range_events, failure_reason};

fn build_stream_with_turns(count: usize) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    for i in 0..count {
        stream.start_turn(format!("turn {i}"));
    }
    stream
}

fn chat_request_texts(events: &[jp_conversation::ConversationEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| e.as_chat_request())
        .map(|r| r.content.clone())
        .collect()
}

#[test]
fn collects_full_range() {
    let stream = build_stream_with_turns(4);
    let events = collect_range_events(&stream, 0, 3);

    assert_eq!(chat_request_texts(&events), vec![
        "turn 0", "turn 1", "turn 2", "turn 3"
    ],);
}

#[test]
fn collects_middle_range_when_range_from_is_nonzero() {
    // Regression: the previous implementation never advanced its turn
    // counter when range_from > 0, so this returned an empty result for
    // any range that didn't start at turn 0 — including the default
    // compaction range (keep_first = 1).
    let stream = build_stream_with_turns(4);
    let events = collect_range_events(&stream, 1, 2);

    assert_eq!(chat_request_texts(&events), vec!["turn 1", "turn 2"]);
}

#[test]
fn collects_default_compaction_range() {
    // Mirrors the default config: keep_first=1, keep_last=1.
    // For a 5-turn stream this keeps turn 0 and turn 4, compacting 1..=3.
    let stream = build_stream_with_turns(5);
    let events = collect_range_events(&stream, 1, 3);

    assert_eq!(chat_request_texts(&events), vec![
        "turn 1", "turn 2", "turn 3"
    ]);
}

#[test]
fn collects_single_turn_at_end() {
    let stream = build_stream_with_turns(4);
    let events = collect_range_events(&stream, 3, 3);

    assert_eq!(chat_request_texts(&events), vec!["turn 3"]);
}

#[test]
fn each_collected_turn_includes_its_turn_start() {
    let stream = build_stream_with_turns(4);
    let events = collect_range_events(&stream, 1, 1);

    // start_turn pushes (TurnStart, ChatRequest), so a single covered
    // turn contributes two events in that order.
    assert_eq!(events.len(), 2);
    assert!(events[0].is_turn_start());
    assert!(events[1].is_chat_request());
}

#[test]
fn empty_for_out_of_bounds_range() {
    let stream = build_stream_with_turns(4);
    let events = collect_range_events(&stream, 10, 20);

    assert!(events.is_empty());
}

#[test]
fn empty_for_empty_stream() {
    let stream = ConversationStream::new_test();
    let events = collect_range_events(&stream, 0, 5);

    assert!(events.is_empty());
}

#[test]
fn refusal_reason_reports_category_and_explanation() {
    let reason = failure_reason(Some(&FinishReason::Refused {
        category: Some("bio".to_owned()),
        explanation: Some("configure a fallback model".to_owned()),
    }));

    assert_eq!(
        reason,
        "the model declined to summarize this conversation (bio): configure a fallback model"
    );
}

#[test]
fn refusal_reason_without_details_is_still_a_refusal() {
    let reason = failure_reason(Some(&FinishReason::Refused {
        category: None,
        explanation: None,
    }));

    assert_eq!(reason, "the model declined to summarize this conversation");
}

#[test]
fn max_tokens_reason_names_the_token_limit() {
    let reason = failure_reason(Some(&FinishReason::MaxTokens));

    assert_eq!(
        reason,
        "the model reached its max output token limit before producing a summary"
    );
}

#[test]
fn other_reason_reports_the_provider_detail() {
    let reason = failure_reason(Some(&FinishReason::Other("content_filter".into())));

    assert_eq!(
        reason,
        "the model stopped early (content_filter) without producing a summary"
    );
}

#[test]
fn clean_finish_with_no_text_is_an_empty_response() {
    // A model that completes normally but emits no message text: the only case
    // where "empty response" is the whole story.
    let reason = failure_reason(Some(&FinishReason::Completed));

    assert_eq!(reason, "the model returned an empty response");
}
