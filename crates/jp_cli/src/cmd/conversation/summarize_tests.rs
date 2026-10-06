use jp_config::model::id::{ModelIdConfig, ProviderId};
use jp_conversation::{
    Compaction, ConversationEvent, ConversationStream, EventKind, PolicySpec, ReasoningPolicy,
    ToolCallPolicy,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse},
};
use jp_llm::{
    event::{Event, EventMatcher, EventPatch, FinishReason, NoticeSink, PatchAction},
    model::ModelDetails,
    provider::mock::MockProvider,
};
use serde_json::Map;

use super::{
    Error, StreamOutcome, build_range_stream, collect_range_events, failure_reason,
    summarize_events, summarize_stream,
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
    summarize_with_ceiling(batches, stream, Some(1_048_576)).await
}

async fn summarize_with_ceiling(
    batches: Vec<Vec<Event>>,
    stream: ConversationStream,
    max_response_bytes: Option<u64>,
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
        &NoticeSink::new(|_| {}),
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

    let error = summarize_with_ceiling(batches, range_stream(&["sig"]), Some(25))
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

/// A request the provider rejects for size is reported as a summarization
/// failure carrying the provider's own numbers.
///
/// The generic stream error would drop the summarizer framing (which model,
/// what to do next) and leave the reader with a bare API complaint.
#[tokio::test]
async fn a_range_the_provider_rejects_for_size_reports_a_summarize_failure() {
    let provider = MockProvider::with_stream_error(
        jp_llm::StreamErrorKind::ContextWindowExceeded,
        "api error: invalid_request_error: prompt is too long: 1318026 tokens > 1000000 maximum",
    );
    let model_id = test_model_id();
    let model_details = ModelDetails::empty(model_id.clone());

    let error = summarize_stream(
        &provider,
        &model_details,
        &model_id,
        range_stream(&["sig"]),
        "instructions",
        "summarize",
        Some(1_048_576),
        &NoticeSink::new(|_| {}),
    )
    .await
    .expect_err("an oversized request must fail");

    let Error::Summarize { model, reason } = error else {
        panic!("expected a summarize failure, got: {error:?}");
    };

    assert_eq!(model, "test/mock-model");
    assert_eq!(
        reason,
        "api error: invalid_request_error: prompt is too long: 1318026 tokens > 1000000 maximum; \
         compact a smaller range (`--from`/`--to`) or summarize with a larger-window model"
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

/// A repair already recorded on the source applies to the summary request too.
///
/// Overlays live beside the events rather than inside them, so a range stream
/// built from conversation events alone replays metadata the provider already
/// rejected and pays for the same repair a second time.
#[test]
fn range_stream_carries_the_source_repairs() {
    let mut source = range_stream(&["stale"]);
    let changed = source.add_overlay(
        [EventPatch {
            matcher: EventMatcher::MetadataValue {
                key: "signature".to_owned(),
                value: "stale".to_owned(),
            },
            action: PatchAction::RemoveMetadata("signature".to_owned()),
        }]
        .iter()
        .map(Into::into)
        .collect(),
    );
    assert_eq!(changed, 1, "the overlay changes the source projection");

    let range = build_range_stream(&source, 0, 0, &Compaction::new(0, 0));

    let signatures: Vec<_> = range
        .iter()
        .filter_map(|e| e.event.metadata.get("signature").cloned())
        .collect();

    assert!(
        signatures.is_empty(),
        "stale signature reaches the summary request: {signatures:?}"
    );
}

/// A turn holding reasoning, a tool call pair, and an answer.
fn turn_with_everything() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("do the thing"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::reasoning("a long deliberation"))
        .add_tool_call_request(ToolCallRequest {
            id: "call-1".to_owned(),
            name: "grep_files".to_owned(),
            arguments: Map::new(),
        })
        .add_tool_call_response(ToolCallResponse {
            id: "call-1".to_owned(),
            result: Ok("three matches".to_owned()),
        })
        .add_chat_response(ChatResponse::message("done"))
        .build()
        .unwrap();
    stream
}

/// The text a summary request would carry, in stream order.
fn request_payloads(stream: &ConversationStream) -> Vec<String> {
    stream
        .iter()
        .filter_map(|e| match &e.event.kind {
            EventKind::ChatRequest(r) => Some(r.content.clone()),
            EventKind::ChatResponse(ChatResponse::Message { message }) => Some(message.clone()),
            EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => {
                Some(reasoning.clone())
            }
            EventKind::ToolCallResponse(r) => r.result.as_ref().ok().cloned(),
            _ => None,
        })
        .collect()
}

/// `--summary` on its own summarizes the turn whole: the reasoning and the tool
/// results are part of what the summary has to stand in for, so they are part
/// of what the summarizer reads.
#[test]
fn a_summary_only_rule_sends_the_whole_turn() {
    let range = build_range_stream(&turn_with_everything(), 0, 0, &Compaction::new(0, 0));

    assert_eq!(request_payloads(&range), vec![
        "do the thing",
        "a long deliberation",
        "three matches",
        "done",
    ]);
}

/// A compaction already on the conversation does not reach the summarizer.
///
/// Stripping reasoning over a range and later summarizing part of it are two
/// decisions, and the second is allowed to disagree with the first: the summary
/// is built from what the turns hold, so it can take the reasoning into account
/// even though the projected conversation no longer shows it.
/// Only the policies of the rule generating this summary narrow what is sent.
#[test]
fn an_existing_compaction_does_not_narrow_what_is_summarized() {
    let mut source = turn_with_everything();
    source.add_compaction(Compaction::new(0, 0).with_reasoning(PolicySpec {
        policy: ReasoningPolicy::Strip,
        over: None,
    }));

    // The projected conversation has lost the reasoning, which is what makes
    // this worth pinning: the summarizer reads the stored events instead.
    let mut projected = source.clone();
    projected.apply_projection();
    assert!(
        !request_payloads(&projected).contains(&"a long deliberation".to_owned()),
        "the existing compaction must strip reasoning from the projection"
    );

    let range = build_range_stream(&source, 0, 0, &Compaction::new(0, 0));

    assert_eq!(request_payloads(&range), vec![
        "do the thing",
        "a long deliberation",
        "three matches",
        "done",
    ]);
}

/// A rule that strips reasoning summarizes a range with no reasoning in it.
///
/// The reported failure: a turn whose reasoning stream alone fills the window
/// could not be summarized, because the request carried the reasoning the same
/// rule was about to discard.
#[test]
fn a_rule_that_strips_reasoning_does_not_send_it() {
    let policies = Compaction::new(0, 0).with_reasoning(PolicySpec {
        policy: ReasoningPolicy::Strip,
        over: None,
    });

    let range = build_range_stream(&turn_with_everything(), 0, 0, &policies);

    assert_eq!(request_payloads(&range), vec![
        "do the thing",
        "three matches",
        "done",
    ]);
}

/// A rule that omits tool calls summarizes a range with no tool calls in it.
#[test]
fn a_rule_that_omits_tool_calls_does_not_send_them() {
    let policies = Compaction::new(0, 0).with_tool_calls(PolicySpec {
        policy: ToolCallPolicy::Omit,
        over: None,
    });

    let range = build_range_stream(&turn_with_everything(), 0, 0, &policies);

    assert_eq!(request_payloads(&range), vec![
        "do the thing",
        "a long deliberation",
        "done",
    ]);
}

/// The policies are renumbered onto the range stream, whose turns start at
/// zero.
///
/// Carrying the source's turn numbers would point the policies past the end of
/// a range that doesn't start at turn 0, silently sending the range unstripped.
#[test]
fn policies_reach_a_range_that_does_not_start_at_turn_zero() {
    let mut source = ConversationStream::new_test();
    source.start_turn(ChatRequest::from("first"));
    source.extend(turn_with_everything().iter().map(|e| e.event.clone()));

    let policies = Compaction::new(1, 1).with_reasoning(PolicySpec {
        policy: ReasoningPolicy::Strip,
        over: None,
    });

    let range = build_range_stream(&source, 1, 1, &policies);

    assert_eq!(request_payloads(&range), vec![
        "do the thing",
        "three matches",
        "done",
    ]);
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
