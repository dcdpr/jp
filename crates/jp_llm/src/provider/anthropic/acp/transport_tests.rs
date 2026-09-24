use std::{error::Error as _, iter};

use async_anthropic::types::JsonOutputFormat;
use datetime_literal::datetime;
use jp_config::{
    AppConfig,
    model::parameters::{CustomReasoningConfig, ReasoningConfig, ReasoningEffort},
};
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ChatResponse, ConversationEvent},
    thread::ThreadBuilder,
};
use jp_tool::InvocationContext;
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    sync::Notify,
    task::JoinHandle,
    time::{advance, timeout},
};
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::{layer::SubscriberExt as _, registry};

use super::{super::recorded_tests::UsageCapture, *};
use crate::event::{EventPart, FinishReason};

fn prepared() -> PreparedRequest {
    prepared_with(None, None)
}

fn prepared_with_limit(max_tokens: Option<u32>) -> PreparedRequest {
    prepared_with(max_tokens, None)
}

fn prepared_with_reasoning(effort: ReasoningEffort) -> PreparedRequest {
    prepared_with(
        None,
        Some(ReasoningConfig::Custom(CustomReasoningConfig {
            effort,
            exclude: false,
        })),
    )
}

fn prepared_with(max_tokens: Option<u32>, reasoning: Option<ReasoningConfig>) -> PreparedRequest {
    let mut config = AppConfig::new_test();
    config.assistant.model.parameters.max_tokens = max_tokens;
    config.assistant.model.parameters.reasoning = reasoning;
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

/// Pushes notifications to JP while a request of its own is still open.
#[derive(Clone)]
struct Notifier(mpsc::UnboundedSender<Value>);

impl Notifier {
    fn notify(&self, method: &str, params: &Value) {
        drop(
            self.0
                .send(json!({"jsonrpc": "2.0", "method": method, "params": params})),
        );
    }

    /// One `_claude/sdkMessage`, the channel Claude Code streams through.
    fn sdk(&self, message: Value) {
        let params = serde_json::to_value(notification(message)).unwrap();
        self.notify(SdkNotification::METHOD, &params);
    }

    fn auth(&self, plan: &str) {
        self.notify(
            AuthUpdate::METHOD,
            &json!({"authStatus": {"kind": "account", "account": {"plan": plan}}}),
        );
    }
}

/// An adapter scripted at the wire level.
///
/// `respond` answers each request JP sends, by method, and may push
/// notifications through its [`Notifier`] before returning.
/// Everything crosses a real pipe as newline-delimited JSON, so the framing is
/// under test rather than bypassed.
fn scripted<F, Fut>(respond: F) -> Box<dyn Transport>
where
    F: Fn(String, Value, Notifier) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Value, RpcError>> + Send + 'static,
{
    Box::new(move |handler, foreground| {
        let (jp_writes, agent_reads) = tokio::io::duplex(1 << 16);
        let (agent_writes, jp_reads) = tokio::io::duplex(1 << 16);
        let (outgoing, mut queued) = mpsc::unbounded_channel::<Value>();
        let notifier = Notifier(outgoing.clone());

        tokio::spawn(async move {
            let mut agent_writes = agent_writes;
            while let Some(message) = queued.recv().await {
                let mut line = serde_json::to_vec(&message).unwrap();
                line.push(b'\n');
                if agent_writes.write_all(&line).await.is_err() {
                    break;
                }
                drop(agent_writes.flush().await);
            }
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(agent_reads).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let message: Value = serde_json::from_str(&line).unwrap();
                let Some(id) = message.get("id").cloned() else {
                    continue;
                };
                let method = message["method"].as_str().unwrap().to_owned();
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let reply = match respond(method, params, notifier.clone()).await {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
                };
                drop(outgoing.send(reply));
            }
        });

        // `.boxed()` rather than `Box::pin`, which infers a future that is not
        // spelled `Send` and so does not satisfy `Transport`.
        futures::FutureExt::boxed(super::super::rpc::drive(
            jp_writes,
            jp_reads,
            Tap::none(),
            handler,
            foreground,
        ))
    })
}

#[tokio::test]
async fn the_scripted_adapter_answers_one_request() {
    let agent = scripted(|method, _params, _notifier| async move {
        assert_eq!(method, agent_method::INITIALIZE);
        Ok(json!({"protocolVersion": 1, "agentCapabilities": {}}))
    });
    let handler: Handler = Box::new(|_| Box::pin(async { Ok(Value::Null) }));
    let foreground: Foreground = Box::new(|peer| {
        Box::pin(async move {
            peer.request(InitializeRequest {
                protocol_version: ProtocolVersion::V1,
                client_capabilities: ClientCapabilities::default(),
            })
            .await?;
            Ok(())
        })
    });
    timeout(Duration::from_secs(5), agent.connect(handler, foreground))
        .await
        .unwrap()
        .unwrap();
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

/// An approval prompt can stay open across a lunch break or a closed laptop,
/// and the adapter's two per-call timers measure wall-clock time regardless of
/// whether JP is scheduled to run.
///
/// A progress notification cannot stand in for this: a heartbeat only resets
/// the timer if it arrives, and a suspended process sends nothing.
#[test]
fn neither_per_call_timer_can_abort_a_call_waiting_on_the_user() {
    const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

    let environment = options::environment(&prepared(), CachePolicy::Short);

    assert_eq!(
        environment
            .get("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT")
            .map(String::as_str),
        Some("0"),
        "the idle check has an off switch, and off is what survives suspension"
    );

    // The wall clock has no off switch, so the ceiling stands in for one: a
    // prompt that outlives it has outlived the 32-bit millisecond timer behind
    // it.
    let ceiling: i64 = environment
        .get("MCP_TOOL_TIMEOUT")
        .expect("a wall-clock ceiling")
        .parse()
        .expect("a plain millisecond count, which is all the adapter parses");
    assert!(
        ceiling >= WEEK_MS,
        "a week is the least a prompt left over a holiday needs, got {ceiling}ms"
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

/// Opus 4.7 and later default `thinking.display` to `omitted`: the model still
/// reasons and still bills for it, but every thinking block comes back empty.
/// JP asks for summarized thinking on the HTTP route, and the subscription
/// route has to ask for the same thing or reasoning silently disappears.
#[test]
fn reasoning_asks_claude_code_for_visible_thinking() {
    let prepared = prepared_with_reasoning(ReasoningEffort::Max);
    let metadata = options::metadata(&prepared, &BTreeMap::new()).unwrap();
    assert_eq!(
        metadata["claudeCode"]["options"]["thinking"],
        json!({"type": "adaptive", "display": "summarized"})
    );
}

/// `effort` and `thinking` are independent options: the effort ladder picks how
/// hard the model works, `thinking.display` picks whether the result is
/// readable.
#[test]
fn reasoning_effort_reaches_the_sdk_options() {
    let prepared = prepared_with_reasoning(ReasoningEffort::Max);
    let metadata = options::metadata(&prepared, &BTreeMap::new()).unwrap();
    assert_eq!(metadata["claudeCode"]["options"]["effort"], json!("max"));
}

/// A structured request reaches Claude Code as its `outputFormat` option.
/// The schema travels as the Anthropic request type rather than a bare map, so
/// this pins the envelope that type serializes into.
#[test]
fn a_structured_request_carries_its_schema_as_the_sdk_output_format() {
    let schema = json!({"type": "object", "properties": {"answer": {"type": "string"}}})
        .as_object()
        .unwrap()
        .clone();
    let mut prepared = prepared();
    prepared.schema = Some(JsonOutputFormat::JsonSchema { schema });

    let metadata = options::metadata(&prepared, &BTreeMap::new()).unwrap();

    assert_eq!(
        metadata["claudeCode"]["options"]["outputFormat"],
        json!({
            "type": "json_schema",
            "schema": {"type": "object", "properties": {"answer": {"type": "string"}}}
        })
    );
}

/// An unstructured request leaves the key out entirely rather than sending
/// `null`, which Claude Code would reject as a malformed output format.
#[test]
fn an_unstructured_request_sends_no_output_format() {
    let metadata = options::metadata(&prepared(), &BTreeMap::new()).unwrap();

    assert!(
        metadata["claudeCode"]["options"]
            .get("outputFormat")
            .is_none()
    );
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
    let agent = scripted(|method, params, _notifier| async move {
        match method.as_str() {
            m if m == agent_method::INITIALIZE => {
                Ok(json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true}}))
            }
            m if m == agent_method::SESSION_LOAD => Ok(json!({})),
            m if m == agent_method::SESSION_SET_CONFIG_OPTION => {
                assert_eq!(params["configId"], "model");
                Err(RpcError::invalid_params().data("Unknown model on this account."))
            }
            other => panic!("unexpected request: {other}"),
        }
    });
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
        assert_eq!(remaining, vec![
            Event::ToolCallPendingEnd {
                id: "call-args".into()
            },
            Event::Finished(FinishReason::Completed)
        ]);
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
    let agent = scripted(move |method, params, notifier| {
        let release = release.clone();
        async move {
            match method.as_str() {
                m if m == agent_method::INITIALIZE => Ok(
                    json!({"protocolVersion": 1, "agentCapabilities": {"mcpCapabilities": {"http": true}}}),
                ),
                m if m == agent_method::SESSION_NEW => {
                    notifier.auth("Claude Max");
                    Ok(json!({"sessionId": "11111111-1111-4111-8111-111111111111"}))
                }
                m if m == agent_method::SESSION_SET_CONFIG_OPTION => Ok(json!({
                    "configOptions": [{
                        "id": params["configId"],
                        "name": "Setting",
                        "type": "select",
                        "currentValue": params["value"],
                        "options": [],
                    }],
                })),
                m if m == agent_method::SESSION_PROMPT => {
                    notifier.sdk(
                        json!({"type": "system", "subtype": "init", "tools": ["mcp__jp__lookup"]}),
                    );
                    notifier.sdk(json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "Rewrite."}}}));
                    notifier.sdk(json!({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}}));
                    if arguments {
                        notifier.sdk(json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "call-args", "name": "mcp__jp__lookup", "input": {}}}}));
                    }
                    release.notified().await;
                    if arguments {
                        notifier.sdk(json!({"type": "stream_event", "event": {"type": "content_block_stop", "index": 1}}));
                    }
                    notifier
                        .sdk(json!({"type": "result", "subtype": "success", "is_error": false}));
                    Ok(json!({"stopReason": "end_turn"}))
                }
                other => panic!("unexpected request: {other}"),
            }
        }
    });
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
    let agent = scripted(|method, params, notifier| async move {
        match method.as_str() {
            m if m == agent_method::INITIALIZE => {
                assert_eq!(params["protocolVersion"], 1);
                Ok(json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {"loadSession": true, "mcpCapabilities": {"http": true}},
                }))
            }
            m if m == agent_method::SESSION_LOAD => {
                assert_eq!(params["sessionId"], "11111111-1111-4111-8111-111111111111");
                assert_eq!(params["cwd"], "/work/project");
                insta::assert_json_snapshot!("session_options", params["_meta"]);
                notifier.auth("Claude Max");
                // Replay of the loaded transcript, which must not reach the
                // caller as live output.
                notifier.sdk(json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "REPLAY MUST NOT APPEAR"}}}));
                Ok(json!({}))
            }
            m if m == agent_method::SESSION_SET_CONFIG_OPTION => {
                let current = if params["configId"] == "model" {
                    assert_eq!(params["value"], "claude-opus-5");
                    json!("resolved-fixture-model")
                } else {
                    params["value"].clone()
                };
                Ok(json!({
                    "configOptions": [{
                        "id": params["configId"],
                        "name": "Setting",
                        "type": "select",
                        "currentValue": current,
                        "options": [],
                    }],
                }))
            }
            m if m == agent_method::SESSION_PROMPT => {
                assert_eq!(
                    params["prompt"],
                    json!([{"type": "text", "text": "Current request."}])
                );
                notifier.sdk(json!({"type": "system", "subtype": "init", "tools": []}));
                notifier.sdk(json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}}));
                notifier.sdk(json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "CURRENT"}}}));
                notifier.sdk(json!({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}}));
                notifier.sdk(json!({"type": "assistant", "message": {"id": "msg-traced", "model": "resolved-fixture-model", "content": [{"type": "text", "text": "CURRENT"}], "usage": {"input_tokens": 2, "output_tokens": 7, "cache_read_input_tokens": 500}}}));
                notifier.sdk(json!({"type": "result", "subtype": "success", "is_error": false}));
                Ok(json!({"stopReason": "end_turn"}))
            }
            other => panic!("unexpected request: {other}"),
        }
    });
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
