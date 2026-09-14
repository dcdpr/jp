use std::{error::Error as _, iter};

use agent_client_protocol::{
    Agent,
    schema::v1::{
        InitializeResponse, LoadSessionResponse, PromptResponse, SessionConfigOptionValue,
        SetSessionConfigOptionResponse,
    },
};
use datetime_literal::datetime;
use jp_config::AppConfig;
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ChatResponse, ConversationEvent},
    thread::ThreadBuilder,
};
use jp_mcp::server::InvocationContext;
use serde_json::{Map, Value, json};

use super::*;
use crate::event::{EventPart, FinishReason};

fn prepared() -> PreparedRequest {
    let timestamp = datetime!(2026-09-11 12:00:00 Z);
    let mut events =
        ConversationStream::new(AppConfig::new_test().into()).with_created_at(timestamp);
    events.extend([
        ConversationEvent::new(ChatRequest::from("Earlier input."), timestamp),
        ConversationEvent::new(ChatResponse::message("Earlier response."), timestamp),
        ConversationEvent::new(ChatRequest::from("Current request."), timestamp),
    ]);
    let thread = ThreadBuilder::new()
        .with_system_prompt("Use JP's history.")
        .with_events(events)
        .build()
        .unwrap();
    let model = super::super::model_details(&"claude-opus-5".parse().unwrap());
    PreparedRequest::new(&model, thread.into()).unwrap()
}

fn notification(message: Value) -> SdkNotification {
    SdkNotification {
        session_id: "11111111-1111-4111-8111-111111111111".into(),
        message: serde_json::from_value(message).unwrap(),
    }
}

#[test]
fn project_directory_is_scoped_by_host_identity_not_worktree_path() {
    let mut context = QueryContext {
        root: "/work/first".into(),
        mcp_endpoint: None,
        invocation: Some(InvocationContext {
            workspace_id: "otvo8".into(),
            conversation_id: "c123456789".into(),
        }),
    };
    assert_eq!(project_name(&context), "jp-c123456789-otvo8");
    context.root = format!("/work/{}", "long".repeat(100)).into();
    assert_eq!(project_name(&context), "jp-c123456789-otvo8");
    context.invocation.as_mut().unwrap().conversation_id = "c987654321".into();
    assert_eq!(project_name(&context), "jp-c987654321-otvo8");
}

#[test]
fn cache_policy_reaches_the_native_sdk_environment() {
    let prepared = prepared();
    for (policy, disabled, ttl) in [
        (CachePolicy::Off, "1", "5m"),
        (CachePolicy::Short, "0", "5m"),
        (CachePolicy::Long, "0", "1h"),
        (CachePolicy::Custom(Duration::from_secs(1799)), "0", "5m"),
        (CachePolicy::Custom(Duration::from_mins(30)), "0", "1h"),
    ] {
        let environment = options::environment(&prepared, policy);
        let metadata = options::metadata(&prepared, &environment).unwrap();
        let native = &metadata["claudeCode"]["options"]["env"];
        assert_eq!(native["DISABLE_PROMPT_CACHING"], disabled);
        assert_eq!(native["CLAUDE_CODE_PROMPT_CACHE_TTL"], ttl);
    }
}

#[test]
fn jp_tool_permissions_always_return_to_the_host() {
    let prepared = prepared();
    let metadata = options::metadata(&prepared, &BTreeMap::new()).unwrap();
    assert_eq!(
        metadata["claudeCode"]["options"]["settings"]["permissions"],
        json!({"ask":["mcp__jp__*"]})
    );
}

#[test]
fn a_followup_error_does_not_replace_the_original_failure() {
    let state = Mutex::new(State::new("haiku".parse().unwrap(), iter::empty(), false));
    record_failure(&state, StreamError::other("Model unavailable."));
    record_failure(&state, StreamError::other("Execution failed."));
    assert_eq!(
        state.into_inner().unwrap().failure.unwrap().message(),
        "Model unavailable."
    );
}

#[tokio::test]
async fn runtime_model_rejection_preserves_its_classification() {
    let agent = Agent
        .builder()
        .on_receive_request(
            async |_request: InitializeRequest, responder, _cx| {
                responder.respond(
                    serde_json::from_value::<InitializeResponse>(
                        json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true}}),
                    )
                    .unwrap(),
                )
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async |_request: LoadSessionRequest, responder, _cx| {
                responder.respond(LoadSessionResponse::new())
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async |request: SetSessionConfigOptionRequest, responder, _cx| {
                assert_eq!(request.config_id.0.as_ref(), "model");
                responder.respond_with_error(
                    RpcError::invalid_params().data("Unknown model on this account."),
                )
            },
            on_receive_request!(),
        );
    let mut prepared = prepared();
    prepared.model = "future-model".parse().unwrap();
    let environment = options::environment(&prepared, CachePolicy::Short);
    let (sender, mut receiver) = mpsc::channel(16);
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        drive(
            prepared,
            QueryContext {
                root: "/work/project".into(),
                mcp_endpoint: None,
                invocation: None,
            },
            vec![],
            environment,
            NativeArtifact {
                session: Some(Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap()),
                path: None,
            },
            agent,
            sender,
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    let Error::Stream(error) = error else {
        panic!("expected classified failure")
    };
    assert!(!error.is_retryable());
    let Some(Error::ModelSelection { model, source }) =
        error.source().unwrap().downcast_ref::<Error>()
    else {
        panic!("expected model selection failure")
    };
    assert_eq!(model.as_ref(), "future-model");
    assert_eq!(
        serde_json::to_value(source).unwrap()["data"],
        "Unknown model on this account."
    );
    assert!(receiver.recv().await.is_none());
}

#[tokio::test]
async fn real_protocol_driver_loads_history_and_emits_only_live_output() {
    let agent = Agent.builder()
        .on_receive_request(async |request: InitializeRequest, responder, _cx| {
            assert_eq!(request.protocol_version, ProtocolVersion::V1);
            let response: InitializeResponse = serde_json::from_value(json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":true}}})).unwrap();
            responder.respond(response)
        }, on_receive_request!())
        .on_receive_request(async |request: LoadSessionRequest, responder, cx| {
            assert_eq!(request.session_id.to_string(), "11111111-1111-4111-8111-111111111111");
            assert_eq!(request.cwd.to_str(), Some("/work/project"));
            insta::assert_json_snapshot!("session_options", request.meta);
            cx.send_notification(serde_json::from_value::<AuthUpdate>(json!({"authStatus":{"kind":"account","account":{"plan":"Claude Max"}}})).unwrap())?;
            cx.send_notification(notification(json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"REPLAY MUST NOT APPEAR"}}})))?;
            responder.respond(LoadSessionResponse::new())
        }, on_receive_request!())
        .on_receive_request(async |request: SetSessionConfigOptionRequest, responder, _cx| {
            if request.config_id.0.as_ref() == "model" {
                assert_eq!(serde_json::to_value(&request.value).unwrap(), json!({"value":"claude-opus-5"}));
            }
            // Request values are flattened objects; a select's current value is the ID itself.
            let SessionConfigOptionValue::ValueId { value } = request.value else { panic!("expected a value ID") };
            let current = if request.config_id.0.as_ref() == "model" { json!("resolved-fixture-model") } else { json!(value) };
            let response: SetSessionConfigOptionResponse = serde_json::from_value(json!({"configOptions":[{"id":request.config_id,"name":"Setting","type":"select","currentValue":current,"options":[]}]})).unwrap();
            responder.respond(response)
        }, on_receive_request!())
        .on_receive_request(async |request: PromptRequest, responder, cx| {
            assert_eq!(serde_json::to_value(request.prompt).unwrap(), json!([{"type":"text","text":"Current request."}]));
            cx.send_notification(notification(json!({"type":"system","subtype":"init","tools":[]})))?;
            cx.send_notification(notification(json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}})))?;
            cx.send_notification(notification(json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"CURRENT"}}})))?;
            cx.send_notification(notification(json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}})))?;
            cx.send_notification(notification(json!({"type":"assistant","message":{"model":"resolved-fixture-model","content":[{"type":"text","text":"CURRENT"}]}})))?;
            cx.send_notification(notification(json!({"type":"result","subtype":"success","is_error":false})))?;
            responder.respond(serde_json::from_value::<PromptResponse>(json!({"stopReason":"end_turn"})).unwrap())
        }, on_receive_request!());
    let prepared = prepared();
    let environment = options::environment(&prepared, CachePolicy::Short);
    let (sender, mut receiver) = mpsc::channel(16);
    let artifact = NativeArtifact {
        session: Some(Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap()),
        path: None,
    };
    tokio::time::timeout(
        Duration::from_secs(5),
        drive(
            prepared,
            QueryContext {
                root: "/work/project".into(),
                mcp_endpoint: None,
                invocation: None,
            },
            vec![],
            environment,
            artifact,
            agent,
            sender,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let mut events = vec![];
    while let Some(event) = receiver.recv().await {
        events.push(event.unwrap());
    }
    // Empty block starts carry no JP content; the first delta supplies it.
    assert_eq!(events, vec![
        Event::Part {
            index: 0,
            part: EventPart::Message("CURRENT".into()),
            metadata: Map::new()
        },
        Event::flush(0),
        Event::Finished(FinishReason::Completed)
    ]);
}
