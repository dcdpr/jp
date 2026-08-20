use jp_conversation::{
    ByteSize, Compaction, PolicySpec, ReasoningPolicy, SummaryPolicy, ToolCallPolicy,
};

use super::*;

#[test]
fn label_detail_item_renders_key_and_value() {
    let item = label_detail_item("branch", "main");

    assert_eq!(item.text, "branch=main");
    assert_eq!(item.json["key"], "branch");
    assert_eq!(item.json["value"], "main");
}

/// A bare label carries an empty value; showing `draft=` would suggest the
/// value is meaningful, so only the key is rendered.
#[test]
fn label_detail_item_renders_a_bare_label_as_the_key_alone() {
    let item = label_detail_item("draft", "");

    assert_eq!(item.text, "draft");
    assert_eq!(item.json["value"], "");
}

#[test]
fn compaction_detail_item_summary_takes_precedence_over_mechanical_label() {
    // A compaction can carry a summary alongside stale reasoning/tool_calls
    // fields (e.g. from an older DSL rule); summary must still win the label.
    let compaction = Compaction::new(0, 4)
        .with_reasoning(ReasoningPolicy::Strip)
        .with_summary(SummaryPolicy {
            summary: "the gist of it".to_owned(),
        });

    let item = compaction_detail_item(&compaction);

    assert_eq!(item.text, "turns 1..5 (5 total, summary)");
    assert_eq!(item.json["from_turn"], 1);
    assert_eq!(item.json["to_turn"], 5);
    assert_eq!(item.json["summary"], "the gist of it");
}

#[test]
fn compaction_detail_item_reports_reasoning_and_tools_policy() {
    let compaction = Compaction::new(2, 2)
        .with_reasoning(ReasoningPolicy::Strip)
        .with_tool_calls(ToolCallPolicy::Strip {
            request: true,
            response: true,
        });

    let item = compaction_detail_item(&compaction);

    // 0-based turn 2 is displayed as turn 3; a single-turn range still reads
    // as an inclusive range for consistency with multi-turn ranges.
    assert_eq!(item.text, "turns 3..3 (1 total, reasoning + tools)");
    assert_eq!(item.json["reasoning"], "strip");
    assert_eq!(item.json["tool_calls"]["policy"], "strip");
    assert!(item.json["summary"].is_null());
}

#[test]
fn compaction_detail_item_with_no_policy_omits_the_label() {
    let compaction = Compaction::new(0, 1);

    let item = compaction_detail_item(&compaction);

    assert_eq!(item.text, "turns 1..2 (2 total)");
}

#[test]
fn compaction_policy_label_describes_partial_tool_call_strip() {
    let request_only = Compaction::new(0, 0).with_tool_calls(ToolCallPolicy::Strip {
        request: true,
        response: false,
    });
    assert_eq!(
        compaction_policy_label(&request_only),
        Some("tool requests".to_owned())
    );

    let response_only = Compaction::new(0, 0).with_tool_calls(ToolCallPolicy::Strip {
        request: false,
        response: true,
    });
    assert_eq!(
        compaction_policy_label(&response_only),
        Some("tool responses".to_owned())
    );

    let omit = Compaction::new(0, 0).with_tool_calls(ToolCallPolicy::Omit);
    assert_eq!(
        compaction_policy_label(&omit),
        Some("tools omitted".to_owned())
    );
}

#[test]
fn compaction_policy_label_is_none_without_any_policy() {
    let compaction = Compaction::new(0, 0);
    assert_eq!(compaction_policy_label(&compaction), None);
}

#[test]
fn compaction_policy_label_names_a_size_threshold() {
    // A rule that only reached the large items must not read the same as one
    // that compacted everything in its range.
    let compaction = Compaction::new(0, 0)
        .with_reasoning(PolicySpec::over(
            ReasoningPolicy::Strip,
            ByteSize::from_bytes(16 * 1024),
        ))
        .with_tool_calls(PolicySpec::over(
            ToolCallPolicy::Strip {
                request: false,
                response: true,
            },
            ByteSize::from_bytes(1024 * 1024),
        ));

    assert_eq!(
        compaction_policy_label(&compaction),
        Some("reasoning over 16KB + tool responses over 1MB".to_owned())
    );
}

#[test]
fn compaction_detail_item_reports_a_size_threshold() {
    let compaction = Compaction::new(0, 0).with_tool_calls(PolicySpec::over(
        ToolCallPolicy::Strip {
            request: false,
            response: true,
        },
        ByteSize::from_bytes(1024 * 1024),
    ));

    let item = compaction_detail_item(&compaction);

    assert_eq!(item.text, "turns 1..1 (1 total, tool responses over 1MB)");
    assert_eq!(item.json["tool_calls"]["policy"], "strip");
    assert_eq!(item.json["tool_calls"]["response"], true);
    assert_eq!(item.json["tool_calls"]["over"], "1MB");
}
