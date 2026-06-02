use indoc::indoc;

use super::{render, render_checklist, render_outline, render_resolution, render_test_results};
use crate::model::{IssueDetail, IssueResolution, Item, ProgressChecklist};

fn outline(json: &str) -> String {
    let items: Vec<Item> = serde_json::from_str(json).unwrap();
    let mut out = String::new();
    render_outline(&items, 0, &mut out);
    out
}

#[test]
fn outline_renders_plain_bullets_with_timestamp() {
    let json = r#"[
        {"content": "plain item", "continuations": []},
        {"timestamp": {"user": {"name": "claude"}, "datetime": "20250101 00:00 UTC"},
         "content": "did a thing", "continuations": []}
    ]"#;

    assert_eq!(outline(json), indoc! {"
        - plain item
        - (claude, 20250101 00:00 UTC) did a thing
    "});
}

#[test]
fn outline_folds_wrapped_paragraphs_and_nests_sublists_and_code() {
    let json = r#"[
        {"content": "parent", "continuations": [
            {"WrappedParagraph": {"content": "continued here"}},
            {"Sublist": {"items": [{"content": "child", "continuations": []}]}},
            {"CodeBlock": {"language": "rust", "content": "fn main() {}"}}
        ]}
    ]"#;

    assert_eq!(outline(json), indoc! {"
        - parent continued here
          - child
          ```rust
          fn main() {}
          ```
    "});
}

#[test]
fn checklist_skips_empty_items_and_marks_completion() {
    let json = r#"{
        "fix_implemented": {"description": "Fix implemented", "completed": true},
        "tests_passing": {"description": "Tests passing", "completed": false},
        "custom_items": [{"description": "Custom thing", "completed": true}]
    }"#;
    let checklist: ProgressChecklist = serde_json::from_str(json).unwrap();

    let mut out = String::new();
    render_checklist(&mut out, &checklist);

    assert_eq!(out, indoc! {"

        ### Checklist

        - [x] Fix implemented
        - [ ] Tests passing
        - [x] Custom thing
    "});
}

#[test]
fn test_results_render_pass_fail() {
    let results = vec![
        ("test_foo".to_string(), true),
        ("test_bar".to_string(), false),
    ];

    let mut out = String::new();
    render_test_results(&mut out, &results);

    assert_eq!(out, indoc! {"

        ### Test Results

        - [pass] test_foo
        - [FAIL] test_bar
    "});
}

#[test]
fn resolution_renders_commit_and_state_variants() {
    let json = r#"[
        {"ClosedWithCommit": {
            "timestamp": {"user": {"name": "rgrant"}, "datetime": "20250101 00:00 UTC"},
            "hash": "abc1234", "comments": null}},
        {"InProgress": {"user": {"name": "bob"}, "datetime": "20250102 00:00 UTC"}}
    ]"#;
    let commits: Vec<IssueResolution> = serde_json::from_str(json).unwrap();

    let mut out = String::new();
    render_resolution(&commits, &mut out);

    assert_eq!(out, indoc! {"
        - Closed in abc1234 (rgrant, 20250101 00:00 UTC)
        - In progress (bob, 20250102 00:00 UTC)
    "});
}

#[test]
fn renders_full_issue_fixture() {
    let json = include_str!("fixtures/issue-220.json");
    let detail: IssueDetail = serde_json::from_str(json).unwrap();
    let rendered = render(&detail);

    // Header.
    assert!(rendered.starts_with("# Issue 220: expect update-context command"));
    assert!(rendered.contains("Source: ag://issues/220"));
    assert!(rendered.contains("File:   done.md"));

    // Populated sections render; null ones do not.
    assert!(rendered.contains("## Description"));
    assert!(rendered.contains("## Implementation Plan"));
    assert!(rendered.contains("## Progress Notes"));
    assert!(rendered.contains("## Debugging"));
    assert!(rendered.contains("## Resolution"));
    assert!(!rendered.contains("## Analysis"));
    assert!(!rendered.contains("## Testing Results"));
    assert!(!rendered.contains("## Implementation Details"));

    // Progress notes keep their per-item timestamp.
    assert!(rendered.contains("- (rgrant, 20251009 22:14 UTC) Naming decision"));

    // The debugging section reconstructs a nested sublist from the
    // continuation tree (two-space indent under its parent bullet).
    assert!(rendered.contains(
        "  - Root cause: From trait conversions always used hardcoded 4-space parent indentation"
    ));

    // Resolution renders the closing commit.
    assert!(rendered.contains("- Closed in 2402e6d (rgrant, 20251010 09:42 UTC)"));
}
