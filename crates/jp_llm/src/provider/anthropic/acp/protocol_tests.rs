use serde_json::json;

use super::*;

fn state() -> State {
    let mut state = State::new("claude-opus-5".into(), ["lookup".into()].into_iter(), false);
    state.session = Some("session-fixed".into());
    state.authenticated = true;
    state.live = true;
    state
        .sdk(notification(
            json!({"type":"system","subtype":"init","tools":["mcp__jp__lookup"]}),
        ))
        .unwrap();
    state
}

fn notification(message: Value) -> SdkNotification {
    SdkNotification {
        session_id: "session-fixed".into(),
        message: serde_json::from_value(message).unwrap(),
    }
}

fn permission() -> RequestPermissionRequest {
    serde_json::from_value(json!({
        "sessionId":"session-fixed",
        "toolCall":{"toolCallId":"tool-fixed","rawInput":{"path":"README.md"},"_meta":{"claudeCode":{"toolName":"mcp__jp__lookup"}}},
        "options":[{"optionId":"once","name":"Allow","kind":"allow_once"}]
    })).unwrap()
}

#[test]
fn dispatch_uses_permission_identity_and_original_arguments() {
    let mut state = state();
    let (response, events) = state.permission(permission()).unwrap();
    assert_eq!(
        serde_json::to_value(response).unwrap(),
        json!({"outcome":{"outcome":"selected","optionId":"once"}})
    );
    assert_eq!(events, vec![
        Event::Part {
            index: 0,
            part: EventPart::ToolCall(ToolCallPart::Start {
                id: "tool-fixed".into(),
                name: "lookup".into()
            }),
            metadata: Map::new()
        },
        Event::Part {
            index: 0,
            part: EventPart::ToolCall(ToolCallPart::ArgumentChunk(
                r#"{"path":"README.md"}"#.into()
            )),
            metadata: Map::new()
        },
        Event::flush(0),
        Event::Finished(FinishReason::Completed)
    ]);
    let error = state.permission(permission()).unwrap_err();
    assert_eq!(
        error.message(),
        "ACP repeated a dispatched tool-call identifier"
    );
}

#[test]
fn permission_uses_the_name_from_the_prior_tool_observation() {
    let mut state = state();
    state.observe(serde_json::from_value(json!({"sessionId":"session-fixed","update":{"sessionUpdate":"tool_call","toolCallId":"tool-fixed","title":"Lookup","_meta":{"claudeCode":{"toolName":"mcp__jp__lookup"}}}})).unwrap());
    let mut request = permission();
    request.tool_call.meta = None;
    let (_, events) = state.permission(request).unwrap();
    assert_eq!(events[0], Event::Part {
        index: 0,
        part: EventPart::ToolCall(ToolCallPart::Start {
            id: "tool-fixed".into(),
            name: "lookup".into()
        }),
        metadata: Map::new()
    });
}

#[test]
fn load_replay_does_not_emit_or_authorize_work() {
    let mut state = state();
    state.live = false;
    let events = state.sdk(notification(json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"OLD"}}}))).unwrap();
    assert_eq!(events, vec![]);
    let error = state.permission(permission()).unwrap_err();
    assert_eq!(
        error.message(),
        "ACP requested tool execution outside an authenticated live request"
    );
}

#[test]
fn unexpected_tools_fail_inventory_check() {
    let mut state = state();
    let error = state
        .sdk(notification(
            json!({"type":"system","subtype":"init","tools":["mcp__jp__lookup","Bash"]}),
        ))
        .unwrap_err();
    assert_eq!(
        error.message(),
        "Claude Code's actual tool inventory differs from JP's configured tools"
    );
}

#[test]
fn sdk_tool_observation_is_not_an_execution_request() {
    let mut state = state();
    let events = state.sdk(notification(json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-fixed","name":"mcp__jp__lookup","input":{}}}}))).unwrap();
    assert_eq!(events, vec![]);
    let events = state.sdk(notification(json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}}))).unwrap();
    assert_eq!(events, vec![]);
}

#[test]
fn synthetic_error_reports_the_reason_not_a_model_mismatch() {
    let mut state = state();
    let error = state.sdk(notification(json!({"type":"assistant","error":"rate_limit","message":{"model":"<synthetic>","content":[{"type":"text","text":"Subscription allowance exhausted."}]}}))).unwrap_err();
    assert_eq!(
        error.message(),
        "Claude Code rate_limit: Subscription allowance exhausted."
    );
}

#[test]
fn sdk_failure_preserves_the_reported_details() {
    let mut state = state();
    let error = state.sdk(notification(json!({"type":"result","subtype":"error_during_execution","is_error":true,"errors":["Adapter disconnected."]}))).unwrap_err();
    assert_eq!(
        error.message(),
        "Claude Code request failed (error_during_execution): Adapter disconnected."
    );
}

#[test]
fn token_limit_is_not_reported_as_completion() {
    let mut state = state();
    state.sdk(notification(json!({"type":"result","subtype":"success","is_error":false,"stop_reason":"max_tokens"}))).unwrap();
    assert_eq!(state.final_events.take().unwrap(), vec![Event::Finished(
        FinishReason::MaxTokens
    )]);
}

#[test]
fn persisted_tool_output_is_reported_instead_of_silently_accepted() {
    let mut state = state();
    let error = state.sdk(notification(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-fixed","content":"<persisted-output>\nOutput too large. Full output saved to: /tmp/result.txt\n</persisted-output>"}]}}))).unwrap_err();
    assert_eq!(
        error.message(),
        "Claude Code replaced tool result tool-fixed with a file reference; the ACP flow cannot \
         preserve this result inline"
    );
}

#[test]
fn successful_subtype_does_not_hide_refusal() {
    let mut state = state();
    state.sdk(notification(json!({"type":"result","subtype":"success","is_error":true,"stop_reason":"refusal","refusal":{"category":"test","explanation":"Refused fixture"}}))).unwrap();
    assert_eq!(state.final_events.take().unwrap(), vec![Event::Finished(
        FinishReason::Refused {
            category: Some("test".into()),
            explanation: Some("Refused fixture".into())
        }
    )]);
}
