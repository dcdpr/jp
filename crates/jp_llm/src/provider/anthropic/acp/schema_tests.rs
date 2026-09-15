//! Every assertion here is the protocol's spelling, written out rather than
//! round-tripped, so a renamed field fails instead of agreeing with itself.

use serde_json::json;

use super::*;

#[test]
fn initialization_states_every_capability_rather_than_omitting_it() {
    let request = InitializeRequest {
        protocol_version: ProtocolVersion::V1,
        client_capabilities: ClientCapabilities::default(),
    };
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false,
            },
        })
    );
}

#[test]
fn an_agent_that_advertises_nothing_reads_as_supporting_nothing() {
    let response: InitializeResponse =
        serde_json::from_value(json!({"protocolVersion": 1})).unwrap();
    assert_eq!(response.protocol_version, ProtocolVersion::V1);
    assert!(!response.agent_capabilities.load_session);
    assert!(!response.agent_capabilities.mcp_capabilities.http);
}

#[test]
fn the_two_capabilities_jp_depends_on_are_read_from_their_own_keys() {
    let response: InitializeResponse = serde_json::from_value(json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {"image": true},
            "mcpCapabilities": {"http": true, "sse": true},
        },
    }))
    .unwrap();
    assert!(response.agent_capabilities.load_session);
    assert!(response.agent_capabilities.mcp_capabilities.http);
}

#[test]
fn a_session_setting_is_sent_without_a_type_discriminator() {
    // The protocol treats a bare `value` as an option id; a `type` here would
    // select a different value shape.
    let request = SetSessionConfigOptionRequest {
        session_id: "sess-1".into(),
        config_id: "model".into(),
        value: "claude-opus-5".into(),
    };
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "sessionId": "sess-1",
            "configId": "model",
            "value": "claude-opus-5",
        })
    );
}

#[test]
fn a_select_setting_reports_the_value_the_agent_settled_on() {
    let response: SetSessionConfigOptionResponse = serde_json::from_value(json!({
        "configOptions": [{
            "id": "model",
            "name": "Model",
            "type": "select",
            "currentValue": "claude-opus-5-20260101",
            "options": [{"value": "claude-opus-5-20260101", "name": "Opus"}],
        }],
    }))
    .unwrap();
    let [option] = response.config_options.as_slice() else {
        panic!("expected one setting")
    };
    assert_eq!(option.id.0, "model");
    let SessionConfigKind::Select { current_value } = &option.kind else {
        panic!("expected a select")
    };
    assert_eq!(current_value.0, "claude-opus-5-20260101");
}

#[test]
fn a_setting_shape_jp_does_not_set_still_decodes() {
    let response: SetSessionConfigOptionResponse = serde_json::from_value(json!({
        "configOptions": [
            {"id": "brave_mode", "name": "Brave", "type": "boolean", "currentValue": true},
            {"id": "future", "name": "Future", "type": "something_new"},
        ],
    }))
    .unwrap();
    assert!(
        response
            .config_options
            .iter()
            .all(|option| matches!(option.kind, SessionConfigKind::Other))
    );
}

#[test]
fn a_new_tool_call_carries_its_id_and_vendor_metadata() {
    let notification: SessionNotification = serde_json::from_value(json!({
        "sessionId": "sess-1",
        "update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-1",
            "title": "Reading configuration",
            "status": "pending",
            "_meta": {"claudeCode": {"toolName": "mcp__jp__lookup"}},
        },
    }))
    .unwrap();
    assert_eq!(notification.session_id.0, "sess-1");
    let SessionUpdate::ToolCall(call) = notification.update else {
        panic!("expected a tool call")
    };
    assert_eq!(call.tool_call_id.0, "tc-1");
    assert_eq!(
        call.meta.unwrap()["claudeCode"]["toolName"],
        "mcp__jp__lookup"
    );
}

#[test]
fn a_tool_call_update_reads_the_fields_the_protocol_flattens() {
    // `status` and `rawInput` live in a nested object in the spec's Rust
    // bindings, but sit beside `toolCallId` on the wire.
    let notification: SessionNotification = serde_json::from_value(json!({
        "sessionId": "sess-1",
        "update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-1",
            "status": "completed",
            "rawInput": {"path": "src/main.rs"},
            "content": [{"type": "content", "content": {"type": "text", "text": "ok"}}],
        },
    }))
    .unwrap();
    let SessionUpdate::ToolCallUpdate(update) = notification.update else {
        panic!("expected a tool call update")
    };
    assert_eq!(update.tool_call_id.0, "tc-1");
    assert_eq!(update.status, Some(ToolCallStatus::Completed));
    assert_eq!(update.raw_input, Some(json!({"path": "src/main.rs"})));
}

#[test]
fn every_other_kind_of_update_decodes_without_failing_the_connection() {
    for update in [
        json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}}),
        json!({"sessionUpdate": "usage_update", "used": 1, "size": 2}),
        json!({"sessionUpdate": "a_kind_added_after_this_build"}),
    ] {
        let notification: SessionNotification =
            serde_json::from_value(json!({"sessionId": "sess-1", "update": update})).unwrap();
        assert!(matches!(notification.update, SessionUpdate::Other));
    }
}

#[test]
fn an_unfinished_tool_call_is_distinguishable_from_a_finished_one() {
    for (wire, expected) in [
        ("pending", ToolCallStatus::Pending),
        ("in_progress", ToolCallStatus::InProgress),
        ("completed", ToolCallStatus::Completed),
        ("failed", ToolCallStatus::Failed),
        ("invented_later", ToolCallStatus::Other),
    ] {
        let status: ToolCallStatus = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(status, expected, "for {wire}");
    }
}

#[test]
fn a_permission_request_offers_its_options_with_their_meaning() {
    let request: RequestPermissionRequest = serde_json::from_value(json!({
        "sessionId": "sess-1",
        "toolCall": {
            "toolCallId": "tc-1",
            "rawInput": {"query": "ripgrep"},
            "_meta": {"claudeCode": {"toolName": "mcp__jp__lookup"}},
        },
        "options": [
            {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
            {"optionId": "no", "name": "Reject", "kind": "reject_once"},
        ],
    }))
    .unwrap();
    assert_eq!(request.session_id.0, "sess-1");
    assert_eq!(request.tool_call.tool_call_id.0, "tc-1");
    assert_eq!(
        request.tool_call.raw_input,
        Some(json!({"query": "ripgrep"}))
    );
    assert_eq!(
        request
            .options
            .iter()
            .map(|option| (option.option_id.0.as_str(), option.kind))
            .collect::<Vec<_>>(),
        [
            ("allow", PermissionOptionKind::AllowOnce),
            ("always", PermissionOptionKind::AllowAlways),
            ("no", PermissionOptionKind::RejectOnce),
        ]
    );
}

#[test]
fn granting_permission_names_the_chosen_option_beside_the_outcome() {
    let response = RequestPermissionResponse {
        outcome: RequestPermissionOutcome::Selected {
            option_id: "allow".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(response).unwrap(),
        json!({
            "outcome": {"outcome": "selected", "optionId": "allow"},
        })
    );
}

#[test]
fn session_setup_requests_name_their_fields_as_the_protocol_does() {
    let new = NewSessionRequest {
        cwd: "/work".into(),
        mcp_servers: vec![json!({"type": "http", "name": "jp", "url": "http://127.0.0.1:1/mcp"})],
        meta: Some(json!({"claudeCode": {}}).as_object().unwrap().clone()),
    };
    assert_eq!(
        serde_json::to_value(new).unwrap(),
        json!({
            "cwd": "/work",
            "mcpServers": [{"type": "http", "name": "jp", "url": "http://127.0.0.1:1/mcp"}],
            "_meta": {"claudeCode": {}},
        })
    );

    let load = LoadSessionRequest {
        session_id: "sess-1".into(),
        cwd: "/work".into(),
        mcp_servers: vec![],
        meta: None,
    };
    assert_eq!(
        serde_json::to_value(load).unwrap(),
        json!({
            "sessionId": "sess-1",
            "cwd": "/work",
            "mcpServers": [],
        })
    );

    let prompt = PromptRequest {
        session_id: "sess-1".into(),
        prompt: vec![json!({"type": "text", "text": "Current request."})],
    };
    assert_eq!(
        serde_json::to_value(prompt).unwrap(),
        json!({
            "sessionId": "sess-1",
            "prompt": [{"type": "text", "text": "Current request."}],
        })
    );
}

#[test]
fn responses_jp_does_not_read_decode_from_whatever_the_agent_sends() {
    serde_json::from_value::<LoadSessionResponse>(json!({})).unwrap();
    serde_json::from_value::<LoadSessionResponse>(json!({"modes": {"currentModeId": "default"}}))
        .unwrap();
    serde_json::from_value::<PromptResponse>(json!({"stopReason": "end_turn"})).unwrap();
    serde_json::from_value::<NewSessionResponse>(json!({
        "sessionId": "sess-1",
        "modes": {"currentModeId": "default", "availableModes": []},
    }))
    .unwrap();
}
