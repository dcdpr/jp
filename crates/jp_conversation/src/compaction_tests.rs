use chrono::{TimeZone as _, Utc};

use super::*;
use crate::ConversationStream;

// ---------------------------------------------------------------------------
// Builder methods
// ---------------------------------------------------------------------------

#[test]
fn builder_with_reasoning() {
    let c = Compaction::new(0, 5).with_reasoning(ReasoningPolicy::Strip);
    assert_eq!(c.reasoning, Some(ReasoningPolicy::Strip.into()));
    assert!(c.tool_calls.is_none());
    assert!(c.summary.is_none());
}

#[test]
fn builder_with_tool_calls() {
    let c = Compaction::new(0, 5).with_tool_calls(ToolCallPolicy::Omit);
    assert_eq!(c.tool_calls, Some(ToolCallPolicy::Omit.into()));
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
        reasoning: Some(ReasoningPolicy::Strip.into()),
        tool_calls: Some(
            ToolCallPolicy::Strip {
                request: true,
                response: true,
            }
            .into(),
        ),
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
        summary: Some(SummaryPolicy::generated(
            "Set up a Rust project with error handling.",
        )),
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
        reasoning: Some(ReasoningPolicy::Strip.into()),
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
    let policy = SummaryPolicy::generated("This is a summary of the conversation.");
    let json = serde_json::to_value(&policy).unwrap();
    assert_eq!(json["summary"], "This is a summary of the conversation.");

    let deserialized: SummaryPolicy = serde_json::from_value(json).unwrap();
    assert_eq!(policy, deserialized);
}

#[test]
fn generated_summary_omits_source_from_json() {
    let json = serde_json::to_value(SummaryPolicy::generated("text")).unwrap();
    let obj = json.as_object().unwrap();

    // Generated is the overwhelmingly common case and the pre-`source` shape,
    // so it stays off the wire.
    assert!(!obj.contains_key("source"));
}

#[test]
fn authored_summary_roundtrip() {
    let policy = SummaryPolicy::authored("I wrote this myself.");
    let json = serde_json::to_value(&policy).unwrap();
    assert_eq!(json["source"], "authored");

    let deserialized: SummaryPolicy = serde_json::from_value(json).unwrap();
    assert_eq!(policy, deserialized);
}

#[test]
fn summary_stored_without_a_source_loads_as_generated() {
    // Every compaction written before `source` existed was model-generated.
    let json = serde_json::json!({ "summary": "stored by an older build" });
    let policy: SummaryPolicy = serde_json::from_value(json).unwrap();

    assert_eq!(policy, SummaryPolicy::generated("stored by an older build"));
}

// ---------------------------------------------------------------------------
// Summary range auto-extension
// ---------------------------------------------------------------------------

fn summary_compaction(from: usize, to: usize, hour: u32) -> Compaction {
    Compaction {
        timestamp: Utc.with_ymd_and_hms(2025, 1, 1, hour, 0, 0).unwrap(),
        from_turn: from,
        to_turn: to,
        summary: Some(SummaryPolicy::generated(format!("summary {from}-{to}"))),
        reasoning: None,
        tool_calls: None,
    }
}

fn authored_compaction(from: usize, to: usize, hour: u32) -> Compaction {
    Compaction {
        timestamp: Utc.with_ymd_and_hms(2025, 1, 1, hour, 0, 0).unwrap(),
        from_turn: from,
        to_turn: to,
        summary: Some(SummaryPolicy::authored(format!("authored {from}-{to}"))),
        reasoning: None,
        tool_calls: None,
    }
}

/// Extend a generated summary over `stream`, expecting no refusal.
fn extend_generated(stream: &ConversationStream, range: CompactionRange) -> CompactionRange {
    extend_summary_range(stream, range, SummarySource::Generated).unwrap()
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
    let result = extend_generated(&stream, range);
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
    let result = extend_generated(&stream, range);
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
    let result = extend_generated(&stream, range);
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
    let result = extend_generated(&stream, range);
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
    let result = extend_generated(&stream, range);
    assert_eq!(result, range);
}

#[test]
fn extend_old_fully_contains_new() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(summary_compaction(0, 9, 10));

    // New [3, 5] sits inside the existing summary [0, 9]. A summary can't be
    // nested inside another, so re-summarizing a contained range refreshes the
    // whole enclosing range: the result grows to [0, 9].
    let range = CompactionRange {
        from_turn: 3,
        to_turn: 5,
    };
    let result = extend_generated(&stream, range);
    assert_eq!(result, CompactionRange {
        from_turn: 0,
        to_turn: 9
    });
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
    let result = extend_generated(&stream, range);
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
        reasoning: Some(ReasoningPolicy::Strip.into()),
        tool_calls: None,
    });

    let range = CompactionRange {
        from_turn: 3,
        to_turn: 7,
    };
    let result = extend_generated(&stream, range);
    assert_eq!(result, range, "Mechanical compactions should be ignored");
}

// ---------------------------------------------------------------------------
// Authored summaries block automatic extension
// ---------------------------------------------------------------------------

#[test]
fn authored_summary_refuses_to_widen_over_an_existing_summary() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(summary_compaction(5, 9, 10));

    // Widening 3..7 to 3..9 would leave the user's text standing in for turns
    // 8 and 9, which they never wrote about.
    let range = CompactionRange {
        from_turn: 3,
        to_turn: 7,
    };
    let error = extend_summary_range(&stream, range, SummarySource::Authored).unwrap_err();

    assert_eq!(error, SummaryOverlap {
        requested: range,
        required: CompactionRange {
            from_turn: 3,
            to_turn: 9
        },
        new_is_authored: true,
    });
}

#[test]
fn authored_summary_covering_the_whole_overlap_is_accepted() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(summary_compaction(3, 5, 10));

    // 0..8 already subsumes the existing summary, so nothing has to grow and
    // the authored text covers exactly the turns the user asked for.
    let range = CompactionRange {
        from_turn: 0,
        to_turn: 8,
    };
    let result = extend_summary_range(&stream, range, SummarySource::Authored).unwrap();

    assert_eq!(result, range);
}

#[test]
fn authored_summary_with_no_overlap_at_all_is_accepted() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(summary_compaction(0, 2, 10));

    let range = CompactionRange {
        from_turn: 5,
        to_turn: 8,
    };
    let result = extend_summary_range(&stream, range, SummarySource::Authored).unwrap();

    assert_eq!(result, range);
}

#[test]
fn generated_summary_refuses_to_widen_over_authored_text() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(authored_compaction(0, 4, 10));

    // Extending 3..8 to 0..8 would replace hand-written text with generated
    // text, beyond the range the user named.
    let range = CompactionRange {
        from_turn: 3,
        to_turn: 8,
    };
    let error = extend_summary_range(&stream, range, SummarySource::Generated).unwrap_err();

    assert_eq!(error, SummaryOverlap {
        requested: range,
        required: CompactionRange {
            from_turn: 0,
            to_turn: 8
        },
        new_is_authored: false,
    });
}

#[test]
fn authored_text_anywhere_in_a_transitive_chain_blocks_extension() {
    let mut stream = stream_with_turns(20);
    // Only C is authored, and it is reached only after two rounds of growth.
    stream.add_compaction(summary_compaction(0, 5, 10));
    stream.add_compaction(summary_compaction(4, 10, 11));
    stream.add_compaction(authored_compaction(9, 15, 12));

    let range = CompactionRange {
        from_turn: 3,
        to_turn: 7,
    };
    let error = extend_summary_range(&stream, range, SummarySource::Generated).unwrap_err();

    assert_eq!(error.required, CompactionRange {
        from_turn: 0,
        to_turn: 15
    });
}

#[test]
fn generated_summary_covering_authored_text_exactly_is_accepted() {
    let mut stream = stream_with_turns(10);
    stream.add_compaction(authored_compaction(3, 5, 10));

    // The user named a range that already subsumes their own summary, so
    // replacing it is explicit rather than incidental.
    let range = CompactionRange {
        from_turn: 0,
        to_turn: 9,
    };
    let result = extend_summary_range(&stream, range, SummarySource::Generated).unwrap();

    assert_eq!(result, range);
}
