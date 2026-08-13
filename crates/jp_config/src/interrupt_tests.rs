use std::str::FromStr as _;

use super::*;
use crate::assignment::KvAssignment;

#[test]
fn compose_in_editor_parses_bools_and_strings() {
    assert_eq!(ComposeInEditor::default(), ComposeInEditor::Inline);
    assert_eq!(ComposeInEditor::from(false), ComposeInEditor::Inline);
    assert_eq!(ComposeInEditor::from(true), ComposeInEditor::Editor);
    assert_eq!(
        ComposeInEditor::from_str("false").unwrap(),
        ComposeInEditor::Inline
    );
    assert_eq!(
        ComposeInEditor::from_str("true").unwrap(),
        ComposeInEditor::Editor
    );
    assert_eq!(
        ComposeInEditor::from_str("always").unwrap(),
        ComposeInEditor::Always
    );
    assert_eq!(
        ComposeInEditor::from_str("never").unwrap(),
        ComposeInEditor::Never
    );
    assert!(ComposeInEditor::from_str("sometimes").is_err());
}

#[test]
fn compose_in_editor_behavior_flags() {
    assert!(!ComposeInEditor::Inline.starts_in_editor());
    assert!(ComposeInEditor::Editor.starts_in_editor());
    assert!(ComposeInEditor::Always.starts_in_editor());
    assert!(!ComposeInEditor::Never.starts_in_editor());

    // `Ctrl+X` escape disabled only for `never`.
    assert!(ComposeInEditor::Inline.editor_escape());
    assert!(ComposeInEditor::Editor.editor_escape());
    assert!(!ComposeInEditor::Never.editor_escape());

    // Only `true` (Editor) falls back to inline on failure.
    assert!(ComposeInEditor::Editor.falls_back_to_inline());
    assert!(!ComposeInEditor::Always.falls_back_to_inline());
}

#[test]
fn compose_in_editor_assigns_from_cli() {
    let mut p = PartialStreamingInterruptConfig::default();
    p.assign(KvAssignment::try_from_cli("compose_in_editor", "always").unwrap())
        .unwrap();
    assert_eq!(p.compose_in_editor, Some(ComposeInEditor::Always));

    p.assign(KvAssignment::try_from_cli("compose_in_editor", "true").unwrap())
        .unwrap();
    assert_eq!(p.compose_in_editor, Some(ComposeInEditor::Editor));
}

#[test]
fn action_defaults_are_prompt() {
    assert_eq!(
        StreamingInterruptAction::default(),
        StreamingInterruptAction::Prompt
    );
    assert_eq!(ToolInterruptAction::default(), ToolInterruptAction::Prompt);
}

#[test]
fn streaming_action_parses_config_values() {
    assert_eq!(
        StreamingInterruptAction::from_str("prompt").unwrap(),
        StreamingInterruptAction::Prompt
    );
    assert_eq!(
        StreamingInterruptAction::from_str("continue").unwrap(),
        StreamingInterruptAction::Continue
    );
    assert_eq!(
        StreamingInterruptAction::from_str("stop").unwrap(),
        StreamingInterruptAction::Stop
    );
    assert_eq!(
        StreamingInterruptAction::from_str("abort").unwrap(),
        StreamingInterruptAction::Abort
    );
    assert_eq!(
        StreamingInterruptAction::from_str("reply").unwrap(),
        StreamingInterruptAction::Reply
    );
}

#[test]
fn tool_action_parses_config_values() {
    assert_eq!(
        ToolInterruptAction::from_str("prompt").unwrap(),
        ToolInterruptAction::Prompt
    );
    assert_eq!(
        ToolInterruptAction::from_str("continue").unwrap(),
        ToolInterruptAction::Continue
    );
    assert_eq!(
        ToolInterruptAction::from_str("restart").unwrap(),
        ToolInterruptAction::Restart
    );
    assert_eq!(
        ToolInterruptAction::from_str("respond").unwrap(),
        ToolInterruptAction::Respond
    );
    assert_eq!(
        ToolInterruptAction::from_str("stop").unwrap(),
        ToolInterruptAction::Stop
    );
}

#[test]
fn unknown_action_is_rejected() {
    assert!(StreamingInterruptAction::from_str("restart").is_err());
    assert!(ToolInterruptAction::from_str("abort").is_err());
}

#[test]
fn assign_sets_nested_action() {
    let mut partial = PartialInterruptConfig::default();
    partial
        .assign(KvAssignment::try_from_cli("streaming.action", "stop").unwrap())
        .unwrap();
    partial
        .assign(KvAssignment::try_from_cli("tool_call.action", "restart").unwrap())
        .unwrap();

    assert_eq!(
        partial.streaming.action,
        Some(StreamingInterruptAction::Stop)
    );
    assert_eq!(partial.tool_call.action, Some(ToolInterruptAction::Restart));
}
