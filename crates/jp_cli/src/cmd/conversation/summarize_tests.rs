use jp_conversation::ConversationStream;
use jp_llm::event::{Event, EventMatcher, EventPatch, FinishReason, PatchAction};

use super::{StreamOutcome, collect_range_events, failure_reason, summarize_events};

/// A stream that produced `text` and then stopped for `reason`.
fn stream_with_text(text: &str, reason: FinishReason) -> Vec<Event> {
    vec![Event::message(0, text), Event::Finished(reason)]
}

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
fn completed_stream_with_text_is_a_summary() {
    let events = vec![
        Event::message(0, "a summary"),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ];

    assert_eq!(
        summarize_events(events),
        StreamOutcome::Summary("a summary".to_owned())
    );
}

#[test]
fn completed_stream_without_text_is_unusable() {
    let events = vec![Event::Finished(FinishReason::Completed)];

    assert_eq!(
        summarize_events(events),
        StreamOutcome::Unusable("the model returned an empty response".to_owned())
    );
}

#[test]
fn truncated_stream_is_unusable_even_though_it_produced_text() {
    // A max-tokens stream normally carries partial text. Returning it would
    // store a truncated summary over the range it replaces, dropping whatever
    // the model never reached.
    let events = stream_with_text("half a summ", FinishReason::MaxTokens);

    assert_eq!(
        summarize_events(events),
        StreamOutcome::Unusable(
            "the model hit its max output token limit, so any summary it produced would be \
             truncated"
                .to_owned()
        )
    );
}

#[test]
fn provider_specific_stop_is_unusable_even_though_it_produced_text() {
    let events = stream_with_text("half a summ", FinishReason::Other("content_filter".into()));

    assert_eq!(
        summarize_events(events),
        StreamOutcome::Unusable(
            "the model stopped early (content_filter), so any summary it produced is incomplete"
                .to_owned()
        )
    );
}

#[test]
fn refusal_is_unusable_even_though_it_produced_text() {
    // `FinishReason::Refused` requires discarding partial output, so text
    // streamed before the decline must not be salvaged.
    let events = stream_with_text("I can help wi", FinishReason::Refused {
        category: Some("bio".to_owned()),
        explanation: Some("configure a fallback model".to_owned()),
    });

    assert_eq!(
        summarize_events(events),
        StreamOutcome::Unusable(
            "the model declined to summarize this conversation (bio): configure a fallback model"
                .to_owned()
        )
    );
}

#[test]
fn retry_hands_back_the_patches_instead_of_a_verdict() {
    // `FinishReason::Retry` means "rebuild the request and resend", not "the
    // model returned nothing". The patches ride along for the rebuild.
    let patch = EventPatch {
        matcher: EventMatcher::MetadataValue {
            key: "signature".to_owned(),
            value: "stale".to_owned(),
        },
        action: PatchAction::RemoveMetadata("signature".to_owned()),
    };

    let events = vec![
        Event::Patch(vec![patch.clone()]),
        Event::Finished(FinishReason::Retry),
    ];

    assert_eq!(summarize_events(events), StreamOutcome::Retry(vec![patch]));
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
fn a_stream_that_never_finished_is_an_empty_response() {
    assert_eq!(failure_reason(None), "the model returned an empty response");
}
