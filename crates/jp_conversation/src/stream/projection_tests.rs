use std::collections::HashSet;

use chrono::{TimeZone as _, Utc};
use proptest::prelude::*;
use serde_json::Map;

use crate::{
    Compaction, ConversationEvent, ConversationStream, EventKind, ReasoningPolicy, SummaryPolicy,
    ToolCallPolicy,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse, TurnStart},
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ts(hour: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 7, 1, hour, 0, 0).unwrap()
}

/// Build a stream with two turns and some tool calls + reasoning.
fn two_turn_stream() -> ConversationStream {
    let mut stream = ConversationStream::new_test();

    // Turn 0
    stream.push(ConversationEvent::new(TurnStart, ts(0)));
    stream.push(ConversationEvent::new(
        ChatRequest::from("set up the project"),
        ts(0),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::reasoning("thinking about setup..."),
        ts(0),
    ));
    stream.push(ConversationEvent::new(
        ToolCallRequest {
            id: "tc1".into(),
            name: "fs_create_file".into(),
            arguments: Map::from_iter([("path".into(), "src/main.rs".into())]),
        },
        ts(0),
    ));
    stream.push(ConversationEvent::new(
        ToolCallResponse {
            id: "tc1".into(),
            result: Ok("file created".into()),
        },
        ts(0),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("Created the project."),
        ts(0),
    ));

    // Turn 1
    stream.push(ConversationEvent::new(TurnStart, ts(1)));
    stream.push(ConversationEvent::new(
        ChatRequest::from("add error handling"),
        ts(1),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::reasoning("considering error types..."),
        ts(1),
    ));
    stream.push(ConversationEvent::new(
        ToolCallRequest {
            id: "tc2".into(),
            name: "fs_modify_file".into(),
            arguments: Map::from_iter([
                ("path".into(), "src/main.rs".into()),
                ("old".into(), "fn main()".into()),
                ("new".into(), "fn main() -> Result<()>".into()),
            ]),
        },
        ts(1),
    ));
    stream.push(ConversationEvent::new(
        ToolCallResponse {
            id: "tc2".into(),
            result: Ok("file modified with 5 changes".into()),
        },
        ts(1),
    ));
    stream.push(ConversationEvent::new(
        ChatResponse::message("Added error handling."),
        ts(1),
    ));

    stream
}

/// Collect only provider-visible events from the stream (what providers see).
fn visible_events(stream: &ConversationStream) -> Vec<&EventKind> {
    stream
        .iter()
        .filter(|e| e.event.kind.is_provider_visible())
        .map(|e| &e.event.kind)
        .collect()
}

// ---------------------------------------------------------------------------
// No compaction → no-op
// ---------------------------------------------------------------------------

#[test]
fn no_compaction_is_noop() {
    let mut stream = two_turn_stream();
    let len_before = stream.len();

    stream.apply_projection();

    assert_eq!(stream.len(), len_before);
}

// ---------------------------------------------------------------------------
// Reasoning strip
// ---------------------------------------------------------------------------

#[test]
fn strip_reasoning_removes_reasoning_events() {
    let mut stream = two_turn_stream();
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: None,
    });

    stream.apply_projection();

    let events = visible_events(&stream);
    assert!(
        !events
            .iter()
            .any(|k| matches!(k, EventKind::ChatResponse(ChatResponse::Reasoning { .. }))),
        "Reasoning events should be stripped"
    );
    // Messages and tool calls remain.
    assert!(
        events
            .iter()
            .any(|k| matches!(k, EventKind::ChatResponse(ChatResponse::Message { .. })))
    );
    assert!(
        events
            .iter()
            .any(|k| matches!(k, EventKind::ToolCallRequest(_)))
    );
}

// ---------------------------------------------------------------------------
// Tool call strip
// ---------------------------------------------------------------------------

#[test]
fn strip_request_only_blanks_args_and_keeps_response() {
    let mut stream = two_turn_stream();
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: false,
        }),
    });

    stream.apply_projection();

    // The request that originally carried arguments is now blanked, but its
    // name and call ID survive — and there's no `[compacted]` marker to echo.
    let req = stream
        .iter()
        .filter_map(|e| e.event.as_tool_call_request().cloned())
        .find(|r| r.id == "tc2")
        .expect("tc2 request present");
    assert!(
        req.arguments.is_empty(),
        "args blanked: {:?}",
        req.arguments
    );
    assert_eq!(req.name, "fs_modify_file", "tool name preserved");

    // The response is left untouched under `strip-requests`.
    let resp = stream.find_tool_call_response("tc2").unwrap();
    assert!(
        !resp.content().starts_with("[compacted]"),
        "response must be preserved: {}",
        resp.content()
    );
}

#[test]
fn strip_tool_calls_replaces_content() {
    let mut stream = two_turn_stream();
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
    });

    stream.apply_projection();

    let events = visible_events(&stream);

    // Tool call requests should have their arguments blanked.
    for kind in &events {
        if let EventKind::ToolCallRequest(req) = kind {
            assert!(
                req.arguments.is_empty(),
                "Request arguments should be blanked: {:?}",
                req.arguments
            );
        }
    }

    // Tool call responses should have compacted content.
    for kind in &events {
        if let EventKind::ToolCallResponse(resp) = kind {
            assert!(
                resp.content().starts_with("[compacted]"),
                "Response should be compacted: {}",
                resp.content()
            );
        }
    }
}

#[test]
fn strip_tool_response_only() {
    let mut stream = two_turn_stream();
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Strip {
            request: false,
            response: true,
        }),
    });

    stream.apply_projection();

    // Requests keep original arguments.
    let req = stream
        .iter()
        .find_map(|e| e.event.as_tool_call_request().cloned())
        .unwrap();
    assert!(
        req.arguments.contains_key("path"),
        "Request arguments should be preserved"
    );

    // Responses are compacted.
    let resp = stream.find_tool_call_response("tc1").unwrap();
    assert!(resp.content().starts_with("[compacted]"));
}

#[test]
fn strip_tool_response_preserves_error_status() {
    let mut stream = ConversationStream::new_test();
    stream.push(ConversationEvent::new(TurnStart, ts(0)));
    stream.push(ConversationEvent::new(
        ChatRequest::from("do something"),
        ts(0),
    ));
    stream.push(ConversationEvent::new(
        ToolCallRequest {
            id: "tc1".into(),
            name: "cargo_test".into(),
            arguments: Map::new(),
        },
        ts(0),
    ));
    stream.push(ConversationEvent::new(
        ToolCallResponse {
            id: "tc1".into(),
            result: Err("test failed: assertion error".into()),
        },
        ts(0),
    ));

    stream.add_compaction(Compaction {
        timestamp: ts(1),
        from_turn: 0,
        to_turn: 0,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Strip {
            request: false,
            response: true,
        }),
    });

    stream.apply_projection();

    let resp = stream.find_tool_call_response("tc1").unwrap();
    assert!(resp.result.is_err(), "Error status should be preserved");
    assert_eq!(resp.content(), "[compacted] cargo_test: error");
}

// ---------------------------------------------------------------------------
// Tool call omit
// ---------------------------------------------------------------------------

#[test]
fn omit_tool_calls_removes_them() {
    let mut stream = two_turn_stream();
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Omit),
    });

    stream.apply_projection();

    let events = visible_events(&stream);
    assert!(
        !events.iter().any(|k| matches!(
            k,
            EventKind::ToolCallRequest(_) | EventKind::ToolCallResponse(_)
        )),
        "All tool call events should be removed"
    );
    // Messages and reasoning still present.
    assert!(
        events
            .iter()
            .any(|k| matches!(k, EventKind::ChatResponse(ChatResponse::Message { .. })))
    );
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

#[test]
fn summary_replaces_all_events_in_range() {
    let mut stream = two_turn_stream();
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: Some(SummaryPolicy {
            summary: "Set up a Rust project with error handling.".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });

    stream.apply_projection();

    let events = visible_events(&stream);

    // Should be exactly: synthetic ChatRequest + synthetic ChatResponse.
    assert_eq!(events.len(), 2, "Summary should produce exactly 2 events");

    assert!(
        matches!(events[0], EventKind::ChatRequest(r) if r.content.contains("Summary")),
        "First event should be the synthetic request"
    );
    assert!(
        matches!(events[1], EventKind::ChatResponse(ChatResponse::Message { message }) if message.contains("error handling")),
        "Second event should be the summary response"
    );
}

#[test]
fn summary_ignores_per_type_policies() {
    let mut stream = two_turn_stream();
    // Both summary and mechanical policies — summary should win.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: Some(SummaryPolicy {
            summary: "Everything summarized.".into(),
        }),
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
    });

    stream.apply_projection();

    let events = visible_events(&stream);
    assert_eq!(
        events.len(),
        2,
        "Summary should replace everything regardless of other policies"
    );
}

#[test]
fn summary_partial_range() {
    let mut stream = two_turn_stream();
    // Only compact turn 0, leave turn 1 intact.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 0,
        summary: Some(SummaryPolicy {
            summary: "Project was set up.".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });

    stream.apply_projection();

    let events = visible_events(&stream);

    // Turn 0: synthetic request + response = 2
    // Turn 1: request + reasoning + tool_req + tool_resp + message = 5
    assert_eq!(events.len(), 7);

    assert!(matches!(
        events[0],
        EventKind::ChatRequest(r) if r.content.contains("Summary")
    ));
    assert!(matches!(
        events[1],
        EventKind::ChatResponse(ChatResponse::Message { message })
        if message.contains("set up")
    ));
    // Turn 1 starts at index 2 with the original ChatRequest.
    assert!(matches!(events[2], EventKind::ChatRequest(r) if r.content == "add error handling"));
}

#[test]
fn summary_is_injected_as_its_own_turn() {
    let mut stream = two_turn_stream();
    // Summarize only the second turn (turn 1).
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 1,
        to_turn: 1,
        summary: Some(SummaryPolicy {
            summary: "summary of turn 1".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });

    stream.apply_projection();

    // The synthetic summary pair carries its own `TurnStart`, so it stays a
    // distinct turn instead of folding into turn 0.
    assert_eq!(stream.turn_count(), 2);
    let turns: Vec<_> = stream.iter_turns().collect();
    let summary_turn = turns.last().unwrap();
    assert!(
        summary_turn.iter().next().unwrap().event.is_turn_start(),
        "summary turn must begin with a TurnStart"
    );
    assert!(summary_turn.iter().any(|e| matches!(
        &e.event.kind,
        EventKind::ChatResponse(ChatResponse::Message { message })
            if message.contains("summary of turn 1")
    )));
}

#[test]
fn distinct_adjacent_summaries_with_identical_text_stay_separate() {
    // Two distinct single-turn summary compactions over adjacent turns that
    // happen to produce identical text must remain two synthetic turns. The old
    // text-only `ResolvedSummary` equality collapsed them into one run, dropping
    // a turn.
    let mut stream = ConversationStream::new_test();
    for t in 0..2 {
        stream.push(ConversationEvent::new(TurnStart, ts(0)));
        stream.push(ConversationEvent::new(
            ChatRequest::from(format!("q{t}")),
            ts(0),
        ));
        stream.push(ConversationEvent::new(
            ChatResponse::message(format!("m{t}")),
            ts(0),
        ));
    }

    stream.add_compaction(Compaction {
        timestamp: ts(1),
        from_turn: 0,
        to_turn: 0,
        summary: Some(SummaryPolicy {
            summary: "SAME".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 1,
        to_turn: 1,
        summary: Some(SummaryPolicy {
            summary: "SAME".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });

    stream.apply_projection();

    assert_eq!(
        stream.turn_count(),
        2,
        "distinct summaries must stay distinct turns"
    );
    let messages: Vec<String> = stream
        .iter()
        .filter_map(|e| match &e.event.kind {
            EventKind::ChatResponse(ChatResponse::Message { message }) => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, vec!["SAME".to_owned(), "SAME".to_owned()]);
}

#[test]
fn contained_summary_reinjects_outer_summary_tail() {
    // Four single-message turns.
    let mut stream = ConversationStream::new_test();
    for t in 0..4 {
        stream.push(ConversationEvent::new(TurnStart, ts(0)));
        stream.push(ConversationEvent::new(
            ChatRequest::from(format!("q{t}")),
            ts(0),
        ));
        stream.push(ConversationEvent::new(
            ChatResponse::message(format!("m{t}")),
            ts(0),
        ));
    }

    // Outer summary over all turns, then a newer summary fully contained in it.
    stream.add_compaction(Compaction {
        timestamp: ts(1),
        from_turn: 0,
        to_turn: 3,
        summary: Some(SummaryPolicy {
            summary: "OUTER".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 1,
        to_turn: 2,
        summary: Some(SummaryPolicy {
            summary: "INNER".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });

    stream.apply_projection();

    let messages: Vec<String> = stream
        .iter()
        .filter_map(|e| match &e.event.kind {
            EventKind::ChatResponse(ChatResponse::Message { message }) => Some(message.clone()),
            _ => None,
        })
        .collect();

    // turn 0 -> OUTER, turns 1..=2 -> INNER, turn 3 -> OUTER again. The outer
    // summary's tail (turn 3) must not be dropped just because OUTER was
    // already injected at turn 0.
    let outer = messages.iter().filter(|m| m.as_str() == "OUTER").count();
    let inner = messages.iter().filter(|m| m.as_str() == "INNER").count();
    assert_eq!(
        outer, 2,
        "outer summary should bracket the inner one: {messages:?}"
    );
    assert_eq!(inner, 1, "{messages:?}");
}

// ---------------------------------------------------------------------------
// Stacking: latest timestamp wins
// ---------------------------------------------------------------------------

#[test]
fn timestamp_tie_breaks_by_stream_order() {
    let mut stream = two_turn_stream();
    // Two compactions over the same turns with the SAME timestamp and
    // conflicting tool policy. The one added later in the stream must win.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Omit),
    });
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
    });

    stream.apply_projection();

    // Later compaction (Strip) wins the tie, so tool calls are present
    // (stripped, not omitted).
    let events = visible_events(&stream);
    assert!(
        events
            .iter()
            .any(|k| matches!(k, EventKind::ToolCallRequest(_))),
        "later compaction in stream order must win the timestamp tie"
    );
}

#[test]
fn later_compaction_wins_for_same_turn() {
    let mut stream = two_turn_stream();

    // Earlier: strip reasoning only.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: None,
    });

    // Later: also strip tool calls.
    stream.add_compaction(Compaction {
        timestamp: ts(3),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
    });

    stream.apply_projection();

    let events = visible_events(&stream);

    // Reasoning should be stripped (from earlier compaction — no later one overrides it).
    assert!(
        !events
            .iter()
            .any(|k| matches!(k, EventKind::ChatResponse(ChatResponse::Reasoning { .. }))),
    );

    // Tool calls should be compacted (from later compaction).
    for kind in &events {
        if let EventKind::ToolCallResponse(resp) = kind {
            assert!(resp.content().starts_with("[compacted]"));
        }
    }
}

#[test]
fn later_compaction_overrides_earlier_for_same_type() {
    let mut stream = two_turn_stream();

    // Earlier: omit tool calls.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Omit),
    });

    // Later: strip tool calls instead (less aggressive).
    stream.add_compaction(Compaction {
        timestamp: ts(3),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
    });

    stream.apply_projection();

    // Tool calls should be stripped (not omitted), because the later compaction wins.
    let events = visible_events(&stream);
    assert!(
        events
            .iter()
            .any(|k| matches!(k, EventKind::ToolCallRequest(_))),
        "Tool calls should be present (stripped, not omitted)"
    );
}

#[test]
fn summary_wins_over_mechanical_for_same_turns() {
    let mut stream = two_turn_stream();

    // Earlier mechanical compaction.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
    });

    // Later summary compaction for the same range.
    stream.add_compaction(Compaction {
        timestamp: ts(3),
        from_turn: 0,
        to_turn: 1,
        summary: Some(SummaryPolicy {
            summary: "All summarized.".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });

    stream.apply_projection();

    let events = visible_events(&stream);
    assert_eq!(events.len(), 2, "Summary should replace everything");
}

// ---------------------------------------------------------------------------
// Stacking: partial overlap
// ---------------------------------------------------------------------------

#[test]
fn compaction_applies_only_to_covered_turns() {
    let mut stream = two_turn_stream();

    // Only compact turn 0.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 0,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: Some(ToolCallPolicy::Omit),
    });

    stream.apply_projection();

    let events = visible_events(&stream);

    // Turn 0: request + message = 2 (reasoning stripped, tools omitted)
    // Turn 1: request + reasoning + tool_req + tool_resp + message = 5
    assert_eq!(events.len(), 7);

    // Turn 1 reasoning should still be present.
    assert!(events.iter().any(|k| matches!(
        k,
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
        if reasoning.contains("error types")
    )));
}

// ---------------------------------------------------------------------------
// Compaction range exceeds actual turn count
// ---------------------------------------------------------------------------

#[test]
fn compaction_beyond_max_turn_is_clamped() {
    let mut stream = two_turn_stream();
    // Range extends beyond existing turns.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 99,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: None,
    });

    stream.apply_projection();

    // Should still work — reasoning stripped from both turns.
    let events = visible_events(&stream);
    assert!(
        !events
            .iter()
            .any(|k| matches!(k, EventKind::ChatResponse(ChatResponse::Reasoning { .. }))),
    );
}

// ---------------------------------------------------------------------------
// Config deltas survive, compaction events consumed
// ---------------------------------------------------------------------------

#[test]
fn config_deltas_preserved_through_projection() {
    let mut stream = two_turn_stream();

    let partial = jp_config::PartialAppConfig::empty();
    stream.add_config_delta(partial);

    let config_before = stream.config().unwrap().to_partial();

    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: Some(SummaryPolicy {
            summary: "all gone".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });

    stream.apply_projection();

    let config_after = stream.config().unwrap().to_partial();
    assert_eq!(
        serde_json::to_value(&config_before).unwrap(),
        serde_json::to_value(&config_after).unwrap(),
    );
}

#[test]
fn compaction_events_consumed_by_projection() {
    let mut stream = two_turn_stream();
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 0,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: None,
    });
    assert_eq!(stream.compactions().count(), 1);

    stream.apply_projection();

    assert_eq!(
        stream.compactions().count(),
        0,
        "Compaction events should be consumed by projection"
    );
}

// ---------------------------------------------------------------------------
// Edge: empty stream
// ---------------------------------------------------------------------------

#[test]
fn empty_stream_with_compaction() {
    let mut stream = ConversationStream::new_test();
    stream.add_compaction(Compaction {
        timestamp: ts(0),
        from_turn: 0,
        to_turn: 0,
        summary: Some(SummaryPolicy {
            summary: "nothing here".into(),
        }),
        reasoning: None,
        tool_calls: None,
    });

    stream.apply_projection();

    assert!(stream.is_empty());
}

// ---------------------------------------------------------------------------
// Re-compaction of projected streams
// ---------------------------------------------------------------------------

/// After projection, old compaction events are gone.
/// Adding a new compaction to the already-projected stream and projecting again
/// should only apply the new compaction — the first projection's effects are
/// baked into the events, and the original compaction doesn't re-apply.
#[test]
fn recompact_projected_stream_with_new_compaction() {
    let mut stream = two_turn_stream();

    // First compaction: strip reasoning from both turns.
    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 0,
        to_turn: 1,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: None,
    });

    stream.apply_projection();

    // Reasoning is gone, tool calls remain.
    assert_eq!(stream.compactions().count(), 0);
    let has_reasoning = stream.iter().any(|e| {
        e.event
            .as_chat_response()
            .is_some_and(ChatResponse::is_reasoning)
    });
    assert!(
        !has_reasoning,
        "Reasoning should be stripped after first projection"
    );
    let tool_call_count = stream
        .iter()
        .filter(|e| e.event.is_tool_call_request())
        .count();
    assert_eq!(
        tool_call_count, 2,
        "Tool calls should survive first projection"
    );

    // Second compaction: now strip tool calls from turn 0 only.
    stream.add_compaction(Compaction {
        timestamp: ts(3),
        from_turn: 0,
        to_turn: 0,
        summary: None,
        reasoning: None,
        tool_calls: Some(ToolCallPolicy::Omit),
    });

    stream.apply_projection();

    // Turn 0 tool calls should now be gone.
    // Turn 1 tool calls should remain (not covered by second compaction).
    let remaining_tool_names: Vec<_> = stream
        .iter()
        .filter_map(|e| e.event.as_tool_call_request().map(|r| r.name.clone()))
        .collect();
    assert_eq!(
        remaining_tool_names,
        vec!["fs_modify_file"],
        "Only turn 1's tool call should remain"
    );

    // Reasoning should still be gone — the first projection already removed
    // it, and no new reasoning-strip compaction was needed.
    let has_reasoning = stream.iter().any(|e| {
        e.event
            .as_chat_response()
            .is_some_and(ChatResponse::is_reasoning)
    });
    assert!(
        !has_reasoning,
        "Reasoning should stay gone after re-compaction"
    );

    // No compaction events should remain.
    assert_eq!(stream.compactions().count(), 0);
}

// ---------------------------------------------------------------------------
// Property tests: arbitrary streams + arbitrary compaction strategies
// ---------------------------------------------------------------------------

/// One turn's optional contents.
/// Every turn also gets a `TurnStart` + request.
#[derive(Debug, Clone)]
struct TurnSpec {
    reasoning: bool,
    tool_calls: usize,
    message: bool,
}

/// A compaction overlay to append, with normalized (`from <= to`) bounds.
#[derive(Debug, Clone)]
struct CompactionSpec {
    from: usize,
    to: usize,
    /// 0=reasoning, 1=strip both, 2=strip request, 3=strip response, 4=omit,
    /// 5=summary.
    policy: u8,
}

fn turn_spec() -> impl Strategy<Value = TurnSpec> {
    (any::<bool>(), 0_usize..3, any::<bool>()).prop_map(|(reasoning, tool_calls, message)| {
        TurnSpec {
            reasoning,
            tool_calls,
            message,
        }
    })
}

fn compaction_spec() -> impl Strategy<Value = CompactionSpec> {
    // Bounds range past the turn count so out-of-range / inverted ranges are
    // exercised too.
    (0_usize..8, 0_usize..8, 0_u8..6).prop_map(|(a, b, policy)| CompactionSpec {
        from: a.min(b),
        to: a.max(b),
        policy,
    })
}

fn build_arbitrary_stream(turns: &[TurnSpec]) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    // Turn event timestamps don't affect projection (turns are delimited by
    // `TurnStart` position, not time), so a constant is fine.
    let at = ts(0);
    for (t, spec) in turns.iter().enumerate() {
        stream.push(ConversationEvent::new(TurnStart, at));
        stream.push(ConversationEvent::new(ChatRequest::from("q"), at));
        if spec.reasoning {
            stream.push(ConversationEvent::new(ChatResponse::reasoning("r"), at));
        }
        for i in 0..spec.tool_calls {
            let id = format!("t{t}c{i}");
            stream.push(ConversationEvent::new(
                ToolCallRequest {
                    id: id.clone(),
                    name: "tool".into(),
                    arguments: Map::from_iter([("k".into(), "v".into())]),
                },
                at,
            ));
            stream.push(ConversationEvent::new(
                ToolCallResponse {
                    id,
                    result: Ok("ok".into()),
                },
                at,
            ));
        }
        if spec.message {
            stream.push(ConversationEvent::new(ChatResponse::message("m"), at));
        }
    }
    stream
}

fn spec_to_compaction(spec: &CompactionSpec) -> Compaction {
    let (summary, reasoning, tool_calls) = match spec.policy {
        0 => (None, Some(ReasoningPolicy::Strip), None),
        1 => (
            None,
            None,
            Some(ToolCallPolicy::Strip {
                request: true,
                response: true,
            }),
        ),
        2 => (
            None,
            None,
            Some(ToolCallPolicy::Strip {
                request: true,
                response: false,
            }),
        ),
        3 => (
            None,
            None,
            Some(ToolCallPolicy::Strip {
                request: false,
                response: true,
            }),
        ),
        4 => (None, None, Some(ToolCallPolicy::Omit)),
        _ => (
            Some(SummaryPolicy {
                summary: "s".into(),
            }),
            None,
            None,
        ),
    };

    Compaction {
        // All compactions share a timestamp; the invariants under test hold
        // regardless of which overlapping policy wins the latest-timestamp
        // contest.
        timestamp: ts(1),
        from_turn: spec.from,
        to_turn: spec.to,
        summary,
        reasoning,
        tool_calls,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// For any stream and any set of compaction overlays, `apply_projection`
    /// must hold these invariants regardless of strategy mix or range:
    ///
    /// - it does not panic,
    /// - it consumes every compaction overlay,
    /// - it leaves no orphaned tool response (one without its request),
    /// - re-applying it to the now-overlay-free stream is a no-op.
    #[test]
    fn projection_holds_invariants(
        turns in proptest::collection::vec(turn_spec(), 0..6),
        specs in proptest::collection::vec(compaction_spec(), 0..5),
    ) {
        let mut stream = build_arbitrary_stream(&turns);
        for spec in &specs {
            stream.add_compaction(spec_to_compaction(spec));
        }

        // No panic (reaching here), and all overlays consumed.
        stream.apply_projection();
        prop_assert_eq!(stream.compactions().count(), 0);

        // No orphaned tool responses: every response keeps a matching request.
        let request_ids: HashSet<String> = stream
            .iter()
            .filter_map(|e| e.event.as_tool_call_request().map(|r| r.id.clone()))
            .collect();
        for e in stream.iter() {
            if let Some(resp) = e.event.as_tool_call_response() {
                prop_assert!(
                    request_ids.contains(&resp.id),
                    "orphaned tool response after projection: {}",
                    resp.id
                );
            }
        }

        // Idempotent: re-projecting an overlay-free stream changes nothing.
        let snapshot = stream.clone();
        stream.apply_projection();
        prop_assert!(stream == snapshot, "re-projection must be a no-op");
    }
}

// ---------------------------------------------------------------------------
// Property test: each strategy applies to exactly its (disjoint) turns
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MechPolicy {
    Reasoning,
    StripBoth,
    StripRequest,
    StripResponse,
    Omit,
}

fn mech_policy() -> impl Strategy<Value = MechPolicy> {
    prop_oneof![
        Just(MechPolicy::Reasoning),
        Just(MechPolicy::StripBoth),
        Just(MechPolicy::StripRequest),
        Just(MechPolicy::StripResponse),
        Just(MechPolicy::Omit),
    ]
}

/// Lay segments out over `[0, num_turns)` as disjoint, ordered ranges with
/// gaps, so every turn is covered by at most one compaction.
fn layout_disjoint(
    num_turns: usize,
    segs: &[(usize, usize, MechPolicy)],
) -> Vec<(usize, usize, MechPolicy)> {
    let mut out = Vec::new();
    let mut cursor: usize = 0;
    for &(gap, len, policy) in segs {
        cursor = cursor.saturating_add(gap);
        if cursor >= num_turns {
            break;
        }
        let to = (cursor + len.max(1) - 1).min(num_turns - 1);
        out.push((cursor, to, policy));
        cursor = to + 1;
    }
    out
}

/// Build a stream whose reasoning text, tool-call IDs, and messages are tagged
/// with their turn index, so each turn's content is locatable in the projected
/// output.
fn build_tagged_stream(turns: &[TurnSpec]) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    let at = ts(0);
    for (t, spec) in turns.iter().enumerate() {
        stream.push(ConversationEvent::new(TurnStart, at));
        stream.push(ConversationEvent::new(
            ChatRequest::from(format!("q{t}")),
            at,
        ));
        if spec.reasoning {
            stream.push(ConversationEvent::new(
                ChatResponse::reasoning(format!("r{t}")),
                at,
            ));
        }
        for i in 0..spec.tool_calls {
            let id = format!("t{t}c{i}");
            stream.push(ConversationEvent::new(
                ToolCallRequest {
                    id: id.clone(),
                    name: "tool".into(),
                    arguments: Map::from_iter([("k".into(), "v".into())]),
                },
                at,
            ));
            stream.push(ConversationEvent::new(
                ToolCallResponse {
                    id,
                    result: Ok("ok".into()),
                },
                at,
            ));
        }
        if spec.message {
            stream.push(ConversationEvent::new(
                ChatResponse::message(format!("m{t}")),
                at,
            ));
        }
    }
    stream
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Each mechanical strategy, applied to a disjoint turn range, must do
    /// exactly what it claims to *its* turns and leave uncovered turns
    /// untouched.
    /// Disjoint ranges keep each turn under a single policy, so the expected
    /// per-turn outcome is known without an oracle.
    /// (Summary is excluded here because it collapses turn structure; it's
    /// covered by the unit tests and the invariant fuzzer.)
    #[test]
    fn each_strategy_applies_to_its_turns(
        turns in proptest::collection::vec(turn_spec(), 0..12),
        segs in proptest::collection::vec((0_usize..3, 1_usize..4, mech_policy()), 0..5),
    ) {
        let num_turns = turns.len();
        let ranges = layout_disjoint(num_turns, &segs);

        let mut policy_by_turn: Vec<Option<MechPolicy>> = vec![None; num_turns];
        let mut stream = build_tagged_stream(&turns);
        for &(from, to, policy) in &ranges {
            for slot in &mut policy_by_turn[from..=to] {
                *slot = Some(policy);
            }
            let (reasoning, tool_calls) = match policy {
                MechPolicy::Reasoning => (Some(ReasoningPolicy::Strip), None),
                MechPolicy::StripBoth => (
                    None,
                    Some(ToolCallPolicy::Strip {
                        request: true,
                        response: true,
                    }),
                ),
                MechPolicy::StripRequest => (
                    None,
                    Some(ToolCallPolicy::Strip {
                        request: true,
                        response: false,
                    }),
                ),
                MechPolicy::StripResponse => (
                    None,
                    Some(ToolCallPolicy::Strip {
                        request: false,
                        response: true,
                    }),
                ),
                MechPolicy::Omit => (None, Some(ToolCallPolicy::Omit)),
            };
            stream.add_compaction(Compaction {
                timestamp: ts(1),
                from_turn: from,
                to_turn: to,
                summary: None,
                reasoning,
                tool_calls,
            });
        }

        stream.apply_projection();

        for t in 0..num_turns {
            let spec = &turns[t];
            let reasoning_tag = format!("r{t}");
            let id_prefix = format!("t{t}c");

            let reasoning_here = stream.iter().any(|e| {
                matches!(
                    &e.event.kind,
                    EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
                        if reasoning == &reasoning_tag
                )
            });
            let requests: Vec<_> = stream
                .iter()
                .filter_map(|e| e.event.as_tool_call_request())
                .filter(|r| r.id.starts_with(&id_prefix))
                .cloned()
                .collect();
            let responses: Vec<_> = stream
                .iter()
                .filter_map(|e| e.event.as_tool_call_response())
                .filter(|r| r.id.starts_with(&id_prefix))
                .cloned()
                .collect();

            match policy_by_turn[t] {
                None => {
                    prop_assert_eq!(reasoning_here, spec.reasoning);
                    prop_assert_eq!(requests.len(), spec.tool_calls);
                    prop_assert!(requests.iter().all(|r| !r.arguments.is_empty()));
                    prop_assert!(responses.iter().all(|r| r.content() == "ok"));
                }
                Some(MechPolicy::Reasoning) => {
                    prop_assert!(!reasoning_here);
                    prop_assert_eq!(requests.len(), spec.tool_calls);
                    prop_assert!(requests.iter().all(|r| !r.arguments.is_empty()));
                    prop_assert!(responses.iter().all(|r| r.content() == "ok"));
                }
                Some(MechPolicy::StripBoth) => {
                    prop_assert_eq!(requests.len(), spec.tool_calls);
                    prop_assert!(requests.iter().all(|r| r.arguments.is_empty()));
                    prop_assert!(responses.iter().all(|r| r.content().starts_with("[compacted]")));
                    prop_assert_eq!(reasoning_here, spec.reasoning);
                }
                Some(MechPolicy::StripRequest) => {
                    prop_assert!(requests.iter().all(|r| r.arguments.is_empty()));
                    prop_assert!(responses.iter().all(|r| r.content() == "ok"));
                }
                Some(MechPolicy::StripResponse) => {
                    prop_assert!(requests.iter().all(|r| !r.arguments.is_empty()));
                    prop_assert!(responses.iter().all(|r| r.content().starts_with("[compacted]")));
                }
                Some(MechPolicy::Omit) => {
                    prop_assert!(requests.is_empty());
                    prop_assert!(responses.is_empty());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Turn index assignment
// ---------------------------------------------------------------------------

#[test]
fn turn_indices_basic() {
    use super::assign_turn_indices;
    use crate::stream::InternalEvent;

    let events = vec![
        InternalEvent::Event(Box::new(ConversationEvent::new(TurnStart, ts(0)))),
        InternalEvent::Event(Box::new(ConversationEvent::new(
            ChatRequest::from("q1"),
            ts(0),
        ))),
        InternalEvent::Event(Box::new(ConversationEvent::new(TurnStart, ts(1)))),
        InternalEvent::Event(Box::new(ConversationEvent::new(
            ChatRequest::from("q2"),
            ts(1),
        ))),
        InternalEvent::Event(Box::new(ConversationEvent::new(TurnStart, ts(2)))),
        InternalEvent::Event(Box::new(ConversationEvent::new(
            ChatRequest::from("q3"),
            ts(2),
        ))),
    ];

    let indices = assign_turn_indices(&events);
    assert_eq!(indices, vec![0, 0, 1, 1, 2, 2]);
}

#[test]
fn turn_indices_with_implicit_leading_turn() {
    use super::assign_turn_indices;
    use crate::stream::InternalEvent;

    // Events before the first `TurnStart` form an implicit turn 0, so the first
    // explicit turn is turn 1 — matching `IterTurns` (see
    // `turn_index_with_implicit_leading_turn` in turn_iter_tests).
    let events = vec![
        InternalEvent::Event(Box::new(ConversationEvent::new(
            ChatRequest::from("orphan"),
            ts(0),
        ))),
        InternalEvent::Event(Box::new(ConversationEvent::new(TurnStart, ts(1)))),
        InternalEvent::Event(Box::new(ConversationEvent::new(
            ChatRequest::from("q1"),
            ts(1),
        ))),
    ];

    let indices = assign_turn_indices(&events);
    assert_eq!(indices, vec![0, 1, 1]);
}

#[test]
fn compaction_targets_correct_turn_with_implicit_leading_turn() {
    // A stream whose first event predates any `TurnStart` has an implicit
    // leading turn 0; the first explicit turn is turn 1. A compaction aimed at
    // turn 1 (built against `iter_turns()` indices) must strip turn 1's
    // reasoning and leave the implicit turn 0 untouched.
    let mut stream = ConversationStream::new_test();
    stream.push(ConversationEvent::new(ChatRequest::from("q0"), ts(0)));
    stream.push(ConversationEvent::new(ChatResponse::reasoning("r0"), ts(0)));
    stream.push(ConversationEvent::new(TurnStart, ts(1)));
    stream.push(ConversationEvent::new(ChatRequest::from("q1"), ts(1)));
    stream.push(ConversationEvent::new(ChatResponse::reasoning("r1"), ts(1)));

    stream.add_compaction(Compaction {
        timestamp: ts(2),
        from_turn: 1,
        to_turn: 1,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: None,
    });

    stream.apply_projection();

    let reasonings: Vec<String> = stream
        .iter()
        .filter_map(|e| match &e.event.kind {
            EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => {
                Some(reasoning.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        reasonings,
        vec!["r0".to_owned()],
        "only turn 1's reasoning should be stripped"
    );
}
