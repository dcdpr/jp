use indexmap::IndexMap;
use jp_conversation::{ConversationStream, event::ChatResponse};

use super::*;
use crate::tool::ToolDocs;

fn tool(name: &str, summary: Option<&str>) -> ToolDefinition {
    ToolDefinition {
        name: name.to_owned(),
        docs: ToolDocs {
            summary: summary.map(str::to_owned),
            ..Default::default()
        },
        parameters: IndexMap::new(),
    }
}

#[test]
fn overhead_empty_inputs() {
    assert_eq!(estimate_overhead_chars(None, &[], &[], &[]), 0);
}

#[test]
fn overhead_system_prompt() {
    let prompt = "You are a helpful assistant.";
    let result = estimate_overhead_chars(Some(prompt), &[], &[], &[]);
    assert_eq!(result, prompt.len());
}

#[test]
fn overhead_sections() {
    let section = SectionConfig::default()
        .with_tag("instruction")
        .with_title("Testing")
        .with_content("Do the thing.");
    let rendered_len = section.render().len();

    let result = estimate_overhead_chars(None, &[section], &[], &[]);
    assert_eq!(result, rendered_len);
}

#[test]
fn overhead_text_attachments() {
    let attachment = Attachment::text("file.rs", "fn main() {}");
    let result = estimate_overhead_chars(None, &[], &[attachment], &[]);
    assert_eq!(result, "fn main() {}".len());
}

#[test]
fn overhead_binary_attachments_ignored() {
    let attachment = Attachment::binary("img.png", vec![0u8; 1000], "image/png");
    let result = estimate_overhead_chars(None, &[], &[attachment], &[]);
    assert_eq!(result, 0);
}

#[test]
fn overhead_tool_definitions() {
    let result =
        estimate_overhead_chars(None, &[], &[], &[tool("grep_files", Some("Search files."))]);
    // name + description + serialized schema
    assert!(result > "grep_files".len() + "Search files.".len());
}

#[test]
fn overhead_combines_all_sources() {
    let prompt = "Be helpful.";
    let section = SectionConfig::default().with_content("Rule 1.");
    let attachment = Attachment::text("f.txt", "hello world");
    let tool = tool("t", None);

    let combined = estimate_overhead_chars(
        Some(prompt),
        std::slice::from_ref(&section),
        std::slice::from_ref(&attachment),
        std::slice::from_ref(&tool),
    );

    let sum = estimate_overhead_chars(Some(prompt), &[], &[], &[])
        + estimate_overhead_chars(None, std::slice::from_ref(&section), &[], &[])
        + estimate_overhead_chars(None, &[], std::slice::from_ref(&attachment), &[])
        + estimate_overhead_chars(None, &[], &[], std::slice::from_ref(&tool));

    assert_eq!(combined, sum);
}

#[test]
fn budget_subtracts_overhead() {
    let no_overhead = budget_chars(1000, 0);
    let with_overhead = budget_chars(1000, 500);
    assert_eq!(no_overhead - 500, with_overhead);
}

#[test]
fn budget_saturates_at_zero() {
    // Overhead larger than total budget shouldn't underflow.
    assert_eq!(budget_chars(100, 999_999), 0);
}

#[test]
fn target_subtracts_overhead() {
    let no_overhead = target_chars(1000, 0);
    let with_overhead = target_chars(1000, 500);
    assert_eq!(no_overhead - 500, with_overhead);
}

#[test]
fn truncate_no_op_when_within_budget() {
    let mut events = ConversationStream::new_test().with_turn("short");
    let count_before = events.len();
    // Large context window, no overhead => no truncation.
    assert_eq!(truncate_to_fit(&mut events, 100_000, 0), 0);
    assert_eq!(events.len(), count_before);
}

#[test]
fn truncate_triggers_with_overhead() {
    // Build a stream that fits in the raw budget but not after subtracting
    // overhead.
    let mut events = ConversationStream::new_test();
    for i in 0..50 {
        events = events.with_turn(format!("message {i} with some padding text here"));
    }

    let total_chars = estimate_chars(&events);
    let count_before = events.len();

    // Pick a context window where total_chars fits at 90% but not after
    // subtracting a large overhead.
    #[expect(clippy::cast_possible_truncation)]
    let context_window = ((total_chars * 100) / (CHARS_PER_TOKEN * OVERHEAD_FACTOR) + 100) as u32;

    // Without overhead, no truncation.
    let mut no_overhead = events.clone();
    assert_eq!(truncate_to_fit(&mut no_overhead, context_window, 0), 0);
    assert_eq!(no_overhead.len(), count_before);

    // With overhead eating most of the budget, truncation should happen.
    let overhead = budget_chars(context_window, 0) - 100;
    let dropped = truncate_to_fit(&mut events, context_window, overhead);
    assert!(dropped > 0);
    assert!(events.len() < count_before);
}

/// A cutoff that drops every chat request but leaves an assistant response
/// behind cannot form a valid provider message sequence, so the whole stream is
/// emptied.
///
/// The sizes are picked so the drop loop stops right after the request: a
/// 3000-char request against a 1000-token window needs 720 chars dropped, which
/// the request alone satisfies, leaving the 100-char response as the only
/// survivor.
#[test]
fn truncate_clears_stream_when_no_chat_request_survives() {
    let mut events = ConversationStream::new_test().with_turn("q".repeat(3000));
    events
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("a".repeat(100)))
        .build()
        .unwrap();

    assert_eq!(truncate_to_fit(&mut events, 1000, 0), 2);
    assert_eq!(events.len(), 0);
}
