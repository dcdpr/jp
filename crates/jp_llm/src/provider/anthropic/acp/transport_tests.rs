use std::{error::Error as _, iter};

use agent_client_protocol::{
    Agent,
    schema::v1::{
        InitializeResponse, LoadSessionResponse, NewSessionResponse, PromptResponse,
        SessionConfigOptionValue, SetSessionConfigOptionResponse,
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
use tokio::{
    sync::Notify,
    task::JoinHandle,
    time::{advance, timeout},
};
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::{layer::SubscriberExt as _, registry};

use super::{super::live_tests::UsageCapture, *};
use crate::event::{EventPart, FinishReason};

fn prepared() -> PreparedRequest {
    prepared_with_limit(None)
}

fn prepared_with_limit(max_tokens: Option<u32>) -> PreparedRequest {
    let mut config = AppConfig::new_test();
    config.assistant.model.parameters.max_tokens = max_tokens;
    let timestamp = datetime!(2026-09-11 12:00:00 Z);
    let mut events = ConversationStream::new(config.into()).with_created_at(timestamp);
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
fn an_unspecified_output_limit_does_not_inherit_the_http_fallback() {
    let environment = options::environment(&prepared(), CachePolicy::Short);
    assert!(!environment.contains_key("CLAUDE_CODE_MAX_OUTPUT_TOKENS"));
}

#[test]
fn an_explicit_output_limit_is_forwarded() {
    let prepared = prepared_with_limit(Some(8192));
    let environment = options::environment(&prepared, CachePolicy::Short);
    assert_eq!(
        environment
            .get("CLAUDE_CODE_MAX_OUTPUT_TOKENS")
            .map(String::as_str),
        Some("8192")
    );
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
fn storage_options_do_not_select_a_different_login_directory() {
    let mut environment = BTreeMap::new();
    configure_storage_environment(&mut environment, None, "jp-c123-otvo8");
    assert!(environment.is_empty());
    configure_storage_environment(&mut environment, Some("/custom/claude"), "jp-c123-otvo8");
    assert_eq!(
        environment,
        BTreeMap::from([(
            "CLAUDE_CODE_PROJECT_DIR_NAME".into(),
            "jp-c123-otvo8".into()
        )])
    );
    assert!(!environment.contains_key("CLAUDE_CONFIG_DIR"));
}

#[test]
fn cache_policy_reaches_the_native_sdk_environment() {
    let prepared = prepared();
    for policy in [
        CachePolicy::Off,
        CachePolicy::Short,
        CachePolicy::Long,
        CachePolicy::Custom(Duration::from_secs(1799)),
        CachePolicy::Custom(Duration::from_mins(30)),
    ] {
        let environment = options::environment(&prepared, policy);
        let metadata = options::metadata(&prepared, &environment).unwrap();
        let native = &metadata["claudeCode"]["options"]["env"];
        if policy == CachePolicy::Off {
            assert_eq!(native["DISABLE_PROMPT_CACHING"], "1");
        } else {
            assert!(native.get("DISABLE_PROMPT_CACHING").is_none());
        }
        assert!(native.get("CLAUDE_CODE_PROMPT_CACHE_TTL").is_none());
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
    let error = timeout(
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

#[tokio::test(start_paused = true)]
async fn buffered_tool_arguments_remain_live_without_dispatching_a_tool() {
    timeout(Duration::from_mins(2), async {
        let LivenessFixture {
            driver,
            mut events,
            finish,
        } = liveness_fixture(true);
        while events.recv().await.unwrap().unwrap() != Event::flush(0) {}
        assert_eq!(
            events.recv().await.unwrap().unwrap(),
            Event::ToolCallPending {
                id: "call-args".into(),
                name: "lookup".into()
            }
        );
        // Hold argument generation open longer than the normal 60-second idle limit.
        for _ in 0..14 {
            advance(Duration::from_secs(5)).await;
            assert_eq!(
                timeout(Duration::from_secs(6), events.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                Event::KeepAlive
            );
        }
        finish.notify_one();
        let mut remaining = vec![];
        while let Some(event) = events.recv().await {
            let event = event.unwrap();
            if event != Event::KeepAlive {
                remaining.push(event);
            }
        }
        assert_eq!(remaining, vec![Event::Finished(FinishReason::Completed)]);
        driver.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_quiet_agent_outside_tool_work_does_not_get_timer_activity() {
    timeout(Duration::from_secs(90), async {
        let LivenessFixture {
            driver,
            mut events,
            finish,
        } = liveness_fixture(false);
        while events.recv().await.unwrap().unwrap() != Event::flush(0) {}
        assert!(
            timeout(Duration::from_secs(65), events.recv())
                .await
                .is_err()
        );
        finish.notify_one();
        let mut remaining = vec![];
        while let Some(event) = events.recv().await {
            let event = event.unwrap();
            if event != Event::KeepAlive {
                remaining.push(event);
            }
        }
        assert_eq!(remaining, vec![Event::Finished(FinishReason::Completed)]);
        driver.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

struct LivenessFixture {
    driver: JoinHandle<Result<(), Error>>,
    events: mpsc::Receiver<Result<Event, StreamError>>,
    finish: Arc<Notify>,
}

fn liveness_fixture(arguments: bool) -> LivenessFixture {
    let finish = Arc::new(Notify::new());
    let release = finish.clone();
    let agent = Agent.builder()
        .on_receive_request(async |_: InitializeRequest, responder, _cx| {
            responder.respond(serde_json::from_value::<InitializeResponse>(json!({"protocolVersion":1,"agentCapabilities":{"mcpCapabilities":{"http":true}}})).unwrap())
        }, on_receive_request!())
        .on_receive_request(async |_: NewSessionRequest, responder, cx| {
            cx.send_notification(serde_json::from_value::<AuthUpdate>(json!({"authStatus":{"kind":"account","account":{"plan":"Claude Max"}}})).unwrap())?;
            responder.respond(serde_json::from_value::<NewSessionResponse>(json!({"sessionId":"11111111-1111-4111-8111-111111111111"})).unwrap())
        }, on_receive_request!())
        .on_receive_request(async |request: SetSessionConfigOptionRequest, responder, _cx| {
            let SessionConfigOptionValue::ValueId { value } = request.value else { panic!("expected value") };
            responder.respond(serde_json::from_value::<SetSessionConfigOptionResponse>(json!({"configOptions":[{"id":request.config_id,"name":"Setting","type":"select","currentValue":value,"options":[]}]})).unwrap())
        }, on_receive_request!())
        .on_receive_request(async move |_: PromptRequest, responder, cx| {
            cx.send_notification(notification(json!({"type":"system","subtype":"init","tools":["mcp__jp__lookup"]})))?;
            cx.send_notification(notification(json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Rewrite."}}})))?;
            cx.send_notification(notification(json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}})))?;
            if arguments {
                cx.send_notification(notification(json!({"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-args","name":"mcp__jp__lookup","input":{}}}})))?;
            }
            release.notified().await;
            if arguments {
                cx.send_notification(notification(json!({"type":"stream_event","event":{"type":"content_block_stop","index":1}})))?;
            }
            cx.send_notification(notification(json!({"type":"result","subtype":"success","is_error":false})))?;
            responder.respond(serde_json::from_value::<PromptResponse>(json!({"stopReason":"end_turn"})).unwrap())
        }, on_receive_request!());
    let prepared = prepared();
    let environment = options::environment(&prepared, CachePolicy::Short);
    let (sender, receiver) = mpsc::channel(16);
    let driver = tokio::spawn(drive(
        prepared,
        QueryContext {
            root: "/work/project".into(),
            mcp_endpoint: Some("http://127.0.0.1:1/mcp".parse().unwrap()),
            invocation: None,
        },
        vec!["lookup".into()],
        environment,
        NativeArtifact {
            session: None,
            path: None,
        },
        agent,
        sender,
    ));
    LivenessFixture {
        driver,
        events: receiver,
        finish,
    }
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
            cx.send_notification(notification(json!({"type":"assistant","message":{"id":"msg-traced","model":"resolved-fixture-model","content":[{"type":"text","text":"CURRENT"}],"usage":{"input_tokens":2,"output_tokens":7,"cache_read_input_tokens":500}}})))?;
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
    let capture = UsageCapture::default();
    let subscriber = registry().with(capture.clone());
    timeout(
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
        )
        .with_subscriber(subscriber),
    )
    .await
    .unwrap()
    .unwrap();
    let mut events = vec![];
    while let Some(event) = receiver.recv().await {
        events.push(event.unwrap());
    }
    assert_eq!(
        capture.snapshot().unwrap(),
        json!({"native_session_id":"11111111-1111-4111-8111-111111111111","requests":{"msg-traced":{"model":"resolved-fixture-model","input_tokens":2,"output_tokens":7,"cache_read_input_tokens":500}}})
    );
    // Replay emits nothing; live SDK observations without content are liveness.
    assert_eq!(events, vec![
        Event::KeepAlive,
        Event::KeepAlive,
        Event::Part {
            index: 0,
            part: EventPart::Message("CURRENT".into()),
            metadata: Map::new()
        },
        Event::flush(0),
        Event::KeepAlive,
        Event::KeepAlive,
        Event::Finished(FinishReason::Completed)
    ]);
}
