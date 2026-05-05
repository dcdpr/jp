use chrono::{TimeZone as _, Utc};
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

    // Tool call requests should have compacted arguments.
    for kind in &events {
        if let EventKind::ToolCallRequest(req) = kind {
            assert!(
                req.arguments.contains_key("[compacted]"),
                "Request arguments should be replaced: {:?}",
                req.arguments
            );
            assert!(
                !req.arguments.contains_key("path"),
                "Original arguments should be gone"
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

// ---------------------------------------------------------------------------
// Stacking: latest timestamp wins
// ---------------------------------------------------------------------------

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

/// After projection, old compaction events are gone. Adding a new compaction
/// to the already-projected stream and projecting again should only apply the
/// new compaction — the first projection's effects are baked into the events,
/// and the original compaction doesn't re-apply.
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
