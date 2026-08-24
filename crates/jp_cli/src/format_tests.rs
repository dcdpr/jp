use jp_conversation::{Compaction, Labels, ReasoningPolicy, SummaryPolicy, ToolCallPolicy};
use jp_term::table::{Details, details};

use super::*;

/// Values are listed beneath their key rather than comma-separated, because a
/// value may contain a comma itself.
#[test]
fn label_detail_item_lists_the_values_beneath_the_key() {
    let values = IndexSet::from(["jp_config".to_owned(), "jp_llm".to_owned()]);
    let item = label_detail_item("crate", &values);

    assert_eq!(item.text, "crate\n    jp_config\n    jp_llm");
    assert_eq!(
        item.json,
        serde_json::json!({ "key": "crate", "values": ["jp_config", "jp_llm"] })
    );
}

/// A bare label is stored as the empty value, which encodes the key's presence
/// rather than saying anything: the key alone is the whole of it.
#[test]
fn label_detail_item_renders_a_bare_label_as_the_key_alone() {
    let values = IndexSet::from([String::new()]);
    let item = label_detail_item("draft", &values);

    assert_eq!(item.text, "draft");
    assert_eq!(
        item.json,
        serde_json::json!({ "key": "draft", "values": [] })
    );
}

/// The empty value stays invisible even where the key holds real ones too.
#[test]
fn label_detail_item_skips_the_empty_value_among_others() {
    let values = IndexSet::from([String::new(), "urgent".to_owned()]);
    let item = label_detail_item("draft", &values);

    assert_eq!(item.text, "draft\n    urgent");
    assert_eq!(
        item.json,
        serde_json::json!({ "key": "draft", "values": ["urgent"] })
    );
}

/// A listing carries the same marker column a mutation's diff does, with a
/// space, so a reader strips one character either way.
#[test]
fn label_lines_carry_a_space_in_the_marker_column() {
    let labels = Labels::from_iter([("crate", vec!["jp_config", "jp_llm"]), ("draft", vec![""])]);

    assert_eq!(label_lines(&labels), [
        " crate=jp_config",
        " crate=jp_llm",
        " draft"
    ]);
}

/// Rendered into a listing, each key stands on its own line with its values
/// beneath it.
#[test]
fn label_items_render_as_a_key_with_its_values_beneath() {
    let labels = Labels::from_iter([("crate", vec!["jp_config", "jp_llm"]), ("draft", vec![""])]);

    let rendered = details(None, Details::Items(label_detail_items(&labels)));

    assert_eq!(rendered, " crate\n     jp_config\n     jp_llm\n draft");
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
    assert!(item.json["reasoning"].as_bool().unwrap());
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
