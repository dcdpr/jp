use std::str::FromStr as _;

use super::*;

#[test]
fn streaming_interrupt_default_is_prompt() {
    assert_eq!(StreamingInterrupt::default(), StreamingInterrupt::Prompt);
}

#[test]
fn tool_interrupt_default_is_prompt() {
    assert_eq!(ToolInterrupt::default(), ToolInterrupt::Prompt);
}

#[test]
fn streaming_interrupt_parses_config_values() {
    assert_eq!(
        StreamingInterrupt::from_str("prompt").unwrap(),
        StreamingInterrupt::Prompt
    );
    assert_eq!(
        StreamingInterrupt::from_str("continue").unwrap(),
        StreamingInterrupt::Continue
    );
    assert_eq!(
        StreamingInterrupt::from_str("stop").unwrap(),
        StreamingInterrupt::Stop
    );
    assert_eq!(
        StreamingInterrupt::from_str("abort").unwrap(),
        StreamingInterrupt::Abort
    );
    assert_eq!(
        StreamingInterrupt::from_str("reply").unwrap(),
        StreamingInterrupt::Reply
    );
}

#[test]
fn tool_interrupt_parses_config_values() {
    assert_eq!(
        ToolInterrupt::from_str("prompt").unwrap(),
        ToolInterrupt::Prompt
    );
    assert_eq!(
        ToolInterrupt::from_str("continue").unwrap(),
        ToolInterrupt::Continue
    );
    assert_eq!(
        ToolInterrupt::from_str("restart").unwrap(),
        ToolInterrupt::Restart
    );
    assert_eq!(
        ToolInterrupt::from_str("stop_reply").unwrap(),
        ToolInterrupt::StopReply
    );
}

#[test]
fn unknown_value_is_rejected() {
    assert!(StreamingInterrupt::from_str("restart").is_err());
    assert!(ToolInterrupt::from_str("abort").is_err());
}

#[test]
fn assign_sets_per_context_action() {
    let mut partial = PartialInterruptConfig::default();
    partial
        .assign(KvAssignment::try_from_cli("streaming", "stop").unwrap())
        .unwrap();
    partial
        .assign(KvAssignment::try_from_cli("tool_call", "restart").unwrap())
        .unwrap();

    assert_eq!(partial.streaming, Some(StreamingInterrupt::Stop));
    assert_eq!(partial.tool_call, Some(ToolInterrupt::Restart));
}
