use std::str::FromStr as _;

use super::*;
use crate::assignment::KvAssignment;

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
        ToolInterruptAction::from_str("stop_reply").unwrap(),
        ToolInterruptAction::StopReply
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
