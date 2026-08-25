use jp_config::model::id::{ModelIdConfig, ProviderId};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse},
};
use jp_llm::{
    event::{Event, EventMatcher, EventPatch, FinishReason, PatchAction},
    model::ModelDetails,
    provider::mock::MockProvider,
};

use super::{
    Error, StreamOutcome, collect_range_events, failure_reason, summarize_events, summarize_stream,
    window_overflow,
};

/// A stream that produced `text` and then stopped for `reason`.
fn stream_with_text(text: &str, reason: FinishReason) -> Vec<Event> {
    vec![Event::message(0, text), Event::Finished(reason)]
}

/// A `Retry` batch carrying a patch that removes `signature = value`.
fn rebuild_request(value: &str) -> Vec<Event> {
    vec![
        Event::Patch(vec![EventPatch {
            matcher: EventMatcher::MetadataValue {
                key: "signature".to_owned(),
                value: value.to_owned(),
            },
            action: PatchAction::RemoveMetadata("signature".to_owned()),
        }]),
        Event::Finished(FinishReason::Retry),
    ]
}

/// A range stream holding one assistant response per signature value.
fn range_stream(signatures: &[&str]) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("a request"));
    stream.extend(signatures.iter().map(|sig| {
        ConversationEvent::now(ChatResponse::message("a response"))
            .with_metadata_field("signature", *sig)
    }));
    stream
}

fn test_model_id() -> ModelIdConfig {
    ModelIdConfig {
        provider: ProviderId::Test,
        name: "mock-model".parse().unwrap(),
    }
}

async fn summarize_with(
    batches: Vec<Vec<Event>>,
    stream: ConversationStream,
) -> super::Result<String> {
    summarize_with_ceiling(batches, stream, 1_048_576).await
}

async fn summarize_with_ceiling(
    batches: Vec<Vec<Event>>,
    stream: ConversationStream,
    max_response_bytes: u32,
) -> super::Result<String> {
    let provider = MockProvider::with_batches(batches);
    let model_id = test_model_id();
    let model_details = ModelDetails::empty(model_id.clone());

    summarize_stream(
        &provider,
        &model_details,
        &model_id,
        stream,
        "instructions",
        "summarize",
        max_response_bytes,
    )
    .await
}

/// A summary request honors the configured ceiling rather than a hardcoded
/// default, and does not re-request the response after breaching it.
#[tokio::test]
async fn summarize_applies_the_configured_output_ceiling() {
    // One scripted batch is deliberate: `MockProvider` panics on a second
    // request, so a ceiling misclassified as retryable fails loudly here.
    // 30 bytes of content against a 25-byte ceiling.
    let batches = vec![stream_with_text(
        "012345678901234567890123456789",
        FinishReason::Completed,
    )];

    let error = summarize_with_ceiling(batches, range_stream(&["sig"]), 25)
        .await
        .expect_err("the summary must stop at the configured ceiling");

    // The default ceiling is 1 MiB; 30 bytes only breaches the configured 25,
    // so reaching this arm proves the setting was threaded through.
    assert!(
        matches!(
            error,
            Error::Llm(jp_llm::Error::Stream(ref e))
                if e.kind == jp_llm::StreamErrorKind::OutputLimit
        ),
        "got: {error:?}"
    );
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

/// A range comfortably inside the window is summarized as-is.
#[test]
fn a_range_that_fits_reports_no_overflow() {
    let stream = build_stream_with_turns(4);
    assert_eq!(window_overflow(&stream, Some(100_000), 0), None);
}

/// The reported failure's shape, on the summarizer path: a large range against
/// a small-window model.
/// Unlike title generation this is rejected rather than shortened, so the
/// summary never covers less than the range it is stored for.
#[test]
fn a_range_past_the_window_overflows() {
    let mut stream = ConversationStream::new_test();
    for i in 0..200 {
        stream.start_turn(format!("turn {i}: {}", "x".repeat(1000)));
    }

    let overflow = window_overflow(&stream, Some(1000), 0).expect("range must not fit");
    assert_eq!(
        overflow,
        "are roughly 201890 characters, which exceeds the ~2700 that fit in the model's 1000 \
         token context window"
    );
}

/// Overhead is charged against the same window, so a range that fits on its own
/// can still overflow once the instructions are counted.
#[test]
fn overhead_can_push_a_fitting_range_over() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("x".repeat(2000));

    assert_eq!(window_overflow(&stream, Some(1000), 0), None);
    assert!(window_overflow(&stream, Some(1000), 1000).is_some());
}

/// Providers that don't report a window (local llama.cpp, Ollama) have no
/// budget to check against, so nothing is rejected.
#[test]
fn an_unknown_window_never_overflows() {
    let mut stream = ConversationStream::new_test();
    for i in 0..200 {
        stream.start_turn(format!("turn {i}: {}", "x".repeat(1000)));
    }

    assert_eq!(window_overflow(&stream, None, 0), None);
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
fn completed_stream_with_only_whitespace_is_unusable() {
    // `EventBuilder::handle_flush` drops a whitespace-only message, so a stream
    // carrying nothing but a newline arrives here with no message at all and
    // takes the same path as one that never produced text.
    //
    // This pins the composed behavior across that boundary, not a check in
    // `summarize_events`: no input can make `summary` non-empty and blank.
    let events = stream_with_text(" \n", FinishReason::Completed);

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

#[tokio::test]
async fn two_sequential_rebuilds_are_honoured() {
    // Anthropic and Google degrade one bad event per `Retry`, oldest first, so a
    // stream with two stale signatures legitimately needs two rounds. A retry
    // ceiling would abort before applying the second patch.
    let summary = summarize_with(
        vec![rebuild_request("one"), rebuild_request("two"), vec![
            Event::message(0, "a summary"),
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ]],
        range_stream(&["one", "two"]),
    )
    .await;

    assert_eq!(summary.unwrap(), "a summary");
}

#[tokio::test]
async fn a_rebuild_that_changes_nothing_stops_the_loop() {
    // The patch matches no event, so resending would fail identically. This is
    // the sole termination guard, so it must fire rather than loop.
    let error = summarize_with(vec![rebuild_request("absent")], range_stream(&["one"]))
        .await
        .expect_err("an unapplicable patch must not be retried");

    assert_eq!(
        error.to_string(),
        "Summarization failed for test/mock-model: the provider asked to rebuild the request but \
         sent no fix that changed it, so resending would fail the same way"
    );
}
