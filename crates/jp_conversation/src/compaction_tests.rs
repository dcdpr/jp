use chrono::{TimeZone as _, Utc};

use super::*;
use crate::ConversationStream;

// ---------------------------------------------------------------------------
// Builder methods
// ---------------------------------------------------------------------------

#[test]
fn builder_with_reasoning() {
    let c = Compaction::new(0, 5).with_reasoning(ReasoningPolicy::Strip);
    assert_eq!(c.reasoning, Some(ReasoningPolicy::Strip));
    assert!(c.tool_calls.is_none());
    assert!(c.summary.is_none());
}

#[test]
fn builder_with_tool_calls() {
    let c = Compaction::new(0, 5).with_tool_calls(ToolCallPolicy::Omit);
    assert_eq!(c.tool_calls, Some(ToolCallPolicy::Omit));
}

#[test]
fn builder_chained() {
    let c = Compaction::new(0, 5)
        .with_reasoning(ReasoningPolicy::Strip)
        .with_tool_calls(ToolCallPolicy::Strip {
            request: true,
            response: true,
        });
    assert!(c.reasoning.is_some());
    assert!(c.tool_calls.is_some());
    assert!(c.summary.is_none());
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

fn sample_compaction() -> Compaction {
    Compaction {
        timestamp: Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
        from_turn: 0,
        to_turn: 5,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: Some(ToolCallPolicy::Strip {
            request: true,
            response: true,
        }),
    }
}

#[test]
fn roundtrip_mechanical_compaction() {
    let original = sample_compaction();
    let json = serde_json::to_value(&original).unwrap();
    let deserialized: Compaction = serde_json::from_value(json).unwrap();
    assert_eq!(original, deserialized);
}

#[test]
fn roundtrip_summary_compaction() {
    let compaction = Compaction {
        timestamp: Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
        from_turn: 0,
        to_turn: 10,
        summary: Some(SummaryPolicy {
            summary: "Set up a Rust project with error handling.".into(),
        }),
        reasoning: None,
        tool_calls: None,
    };

    let json = serde_json::to_value(&compaction).unwrap();
    let deserialized: Compaction = serde_json::from_value(json).unwrap();
    assert_eq!(compaction, deserialized);
}

#[test]
fn none_policies_omitted_from_json() {
    let compaction = Compaction {
        timestamp: Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
        from_turn: 0,
        to_turn: 3,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: None,
    };

    let json = serde_json::to_value(&compaction).unwrap();
    let obj = json.as_object().unwrap();

    assert!(!obj.contains_key("summary"));
    assert!(obj.contains_key("reasoning"));
    assert!(!obj.contains_key("tool_calls"));
}

#[test]
fn tool_call_policy_strip_roundtrip() {
    let policy = ToolCallPolicy::Strip {
        request: false,
        response: true,
    };
    let json = serde_json::to_value(&policy).unwrap();
    assert_eq!(json["policy"], "strip");
    assert_eq!(json["request"], false);
    assert_eq!(json["response"], true);

    let deserialized: ToolCallPolicy = serde_json::from_value(json).unwrap();
    assert_eq!(policy, deserialized);
}

#[test]
fn tool_call_policy_omit_roundtrip() {
    let policy = ToolCallPolicy::Omit;
    let json = serde_json::to_value(&policy).unwrap();
    assert_eq!(json["policy"], "omit");

    let deserialized: ToolCallPolicy = serde_json::from_value(json).unwrap();
    assert_eq!(policy, deserialized);
}

#[test]
fn reasoning_policy_roundtrip() {
    let policy = ReasoningPolicy::Strip;
    let json = serde_json::to_value(&policy).unwrap();
    assert_eq!(json, serde_json::json!("strip"));

    let deserialized: ReasoningPolicy = serde_json::from_value(json).unwrap();
    assert_eq!(policy, deserialized);
}

#[test]
fn summary_policy_roundtrip() {
    let policy = SummaryPolicy {
        summary: "This is a summary of the conversation.".into(),
    };
    let json = serde_json::to_value(&policy).unwrap();
    assert_eq!(json["summary"], "This is a summary of the conversation.");

    let deserialized: SummaryPolicy = serde_json::from_value(json).unwrap();
    assert_eq!(policy, deserialized);
}

// ---------------------------------------------------------------------------
// Summary range auto-extension
// ---------------------------------------------------------------------------

fn summary_compaction(from: usize, to: usize, hour: u32) -> Compaction {
    Compaction {
        timestamp: Utc.with_ymd_and_hms(2025, 1, 1, hour, 0, 0).unwrap(),
        from_turn: from,
        to_turn: to,
        summary: Some(SummaryPolicy {
            summary: format!("summary {from}-{to}"),
        }),
        reasoning: None,
        tool_calls: None,
    }
}

/// Build a stream with `n` turns.
#[expect(clippy::cast_possible_truncation)]
fn stream_with_turns(n: usize) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    for i in 0..n {
        stream.extend(vec![
            crate::ConversationEvent::new(
                crate::event::TurnStart,
                Utc.with_ymd_and_hms(2025, 1, 1, i as u32, 0, 0).unwrap(),
            ),
            crate::ConversationEvent::new(
                crate::event::ChatRequest::from(format!("turn {i}")),
                Utc.with_ymd_and_hms(2025, 1, 1, i as u32, 0, 1).unwrap(),
            ),
        ]);
    }
    stream
}

#[test]
fn extend_no_existing_summaries() {
    let stream = stream_with_turns(10);
    let range = CompactionRange {
        from_turn: 3,
        to_turn: 7,
    };
    let result = extend_summary_range(&stream, range);
    assert_eq!(result, range, "No existing summaries → unchanged");
}

#[test]
fn extend_no_overlap() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(summary_compaction(0, 2, 10));

    let range = CompactionRange {
        from_turn: 5,
        to_turn: 8,
    };
    let result = extend_summary_range(&stream, range);
    assert_eq!(result, range, "Disjoint ranges → unchanged");
}

#[test]
fn extend_partial_overlap_right() {
    let mut stream = stream_with_turns(10);
    // Existing summary: turns 5–10.
    stream.add_compaction(summary_compaction(5, 9, 10));

    // New range 3–7 partially overlaps: extends to 3–9.
    let range = CompactionRange {
        from_turn: 3,
        to_turn: 7,
    };
    let result = extend_summary_range(&stream, range);
    assert_eq!(result, CompactionRange {
        from_turn: 3,
        to_turn: 9
    });
}

#[test]
fn extend_partial_overlap_left() {
    let mut stream = stream_with_turns(10);
    // Existing summary: turns 0–4.
    stream.add_compaction(summary_compaction(0, 4, 10));

    // New range 3–8 partially overlaps: extends to 0–8.
    let range = CompactionRange {
        from_turn: 3,
        to_turn: 8,
    };
    let result = extend_summary_range(&stream, range);
    assert_eq!(result, CompactionRange {
        from_turn: 0,
        to_turn: 8
    });
}

#[test]
fn extend_new_fully_contains_old() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(summary_compaction(3, 5, 10));

    // New [0, 8] fully contains old [3, 5] → no extension needed.
    let range = CompactionRange {
        from_turn: 0,
        to_turn: 8,
    };
    let result = extend_summary_range(&stream, range);
    assert_eq!(result, range);
}

#[test]
fn extend_old_fully_contains_new() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(summary_compaction(0, 9, 10));

    // New [3, 5] fully contained by old [0, 9] → no extension.
    let range = CompactionRange {
        from_turn: 3,
        to_turn: 5,
    };
    let result = extend_summary_range(&stream, range);
    assert_eq!(result, range);
}

#[test]
fn extend_transitive_chain() {
    let mut stream = stream_with_turns(20);
    // A: 0–5, B: 4–10, C: 9–15
    stream.add_compaction(summary_compaction(0, 5, 10));
    stream.add_compaction(summary_compaction(4, 10, 11));
    stream.add_compaction(summary_compaction(9, 15, 12));

    // New range 3–7 overlaps A and B directly.
    // After extending to 0–10, that overlaps C → extends to 0–15.
    let range = CompactionRange {
        from_turn: 3,
        to_turn: 7,
    };
    let result = extend_summary_range(&stream, range);
    assert_eq!(result, CompactionRange {
        from_turn: 0,
        to_turn: 15
    });
}

#[test]
fn extend_ignores_mechanical_compactions() {
    let mut stream = stream_with_turns(10);
    // Mechanical compaction (no summary) covering 0–9.
    stream.add_compaction(Compaction {
        timestamp: Utc.with_ymd_and_hms(2025, 1, 1, 10, 0, 0).unwrap(),
        from_turn: 0,
        to_turn: 9,
        summary: None,
        reasoning: Some(ReasoningPolicy::Strip),
        tool_calls: None,
    });

    let range = CompactionRange {
        from_turn: 3,
        to_turn: 7,
    };
    let result = extend_summary_range(&stream, range);
    assert_eq!(result, range, "Mechanical compactions should be ignored");
}
