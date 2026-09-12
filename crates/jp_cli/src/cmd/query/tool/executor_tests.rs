use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_mcp::server::BuiltinTool;
use jp_tool::{Outcome, ToolDocs};
use serde_json::json;
use tokio::time::{Duration, advance, pause, resume, timeout};

use super::*;

struct InquiringTool(Arc<AtomicUsize>);
#[async_trait]
impl BuiltinTool for InquiringTool {
    async fn execute(&self, arguments: &Value, answers: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        if let Some(answer) = answers.get("confirm") {
            return Outcome::Success {
                content: json!({"arguments":arguments,"answer":answer}).to_string(),
            };
        }
        Question::boolean("confirm", "Continue?").unwrap().into()
    }
}

#[tokio::test]
async fn http_executor_keeps_one_call_through_input_and_recording() {
    let mut cfg = AppConfig::new_test();
    let partial: PartialToolConfig =
        serde_json::from_value(json!({"source":"builtin", "run":"ask", "result":"edit"})).unwrap();
    cfg.conversation.tools.insert(
        "example".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let count = Arc::new(AtomicUsize::new(0));
    let definitions = vec![ToolDefinition {
        name: "example".into(),
        docs: ToolDocs::default(),
        parameters: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
    }];
    let (source, owner) = TerminalExecutorSource::start(
        BuiltinExecutors::new().register("example", InquiringTool(count.clone())),
        &definitions,
        &cfg.conversation.tools,
        Arc::new(ApprovalStore::default()),
        InvocationContext::default(),
        &Client::default(),
        "/tmp".into(),
    )
    .await
    .unwrap();
    let mut executor = source
        .create(
            ToolCallRequest {
                id: "call-1".into(),
                name: "example".into(),
                arguments: json!({"name":"original"}).as_object().unwrap().clone(),
            },
            cfg.conversation.tools.get("example").unwrap(),
        )
        .unwrap();
    assert_eq!(executor.prepare(false).await.unwrap(), None);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    executor.set_arguments(json!({"name":"edited"}));
    executor.approve().await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let first = executor
        .execute(
            &IndexMap::new(),
            &Client::default(),
            "/tmp".into(),
            CancellationToken::new(),
            None,
        )
        .await;
    let ExecutorResult::NeedsInput { question, .. } = first else {
        panic!("expected question")
    };
    assert_eq!(question, Question::boolean("confirm", "Continue?").unwrap());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let second = executor
        .execute(
            &IndexMap::from_iter([("confirm".into(), json!(true))]),
            &Client::default(),
            "/tmp".into(),
            CancellationToken::new(),
            None,
        )
        .await;
    let ExecutorResult::Completed(response) = second else {
        panic!("expected result review")
    };
    assert_eq!(
        response.result,
        Ok(r#"{"arguments":{"name":"edited"},"answer":true}"#.into())
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    timeout(
        Duration::from_secs(2),
        source.acknowledge(ToolCallResponse {
            id: "call-1".into(),
            result: Ok("reviewed".into()),
        }),
    )
    .await
    .unwrap()
    .unwrap();
    owner.shutdown().await.unwrap();
}

async fn fixture(
    result_mode: &str,
) -> (
    TerminalExecutorSource,
    ExecutionOwner,
    Box<dyn Executor>,
    Arc<AtomicUsize>,
) {
    let mut cfg = AppConfig::new_test();
    let partial: PartialToolConfig =
        serde_json::from_value(json!({"source":"builtin", "run":"ask", "result":result_mode}))
            .unwrap();
    cfg.conversation.tools.insert(
        "example".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let count = Arc::new(AtomicUsize::new(0));
    let definitions = vec![ToolDefinition {
        name: "example".into(),
        docs: ToolDocs::default(),
        parameters: json!({"type":"object","properties":{}}),
    }];
    let (source, owner) = TerminalExecutorSource::start(
        BuiltinExecutors::new().register("example", InquiringTool(count.clone())),
        &definitions,
        &cfg.conversation.tools,
        Arc::new(ApprovalStore::default()),
        InvocationContext::default(),
        &Client::default(),
        "/tmp".into(),
    )
    .await
    .unwrap();
    let executor = source
        .create(
            ToolCallRequest {
                id: "call-1".into(),
                name: "example".into(),
                arguments: Map::new(),
            },
            cfg.conversation.tools.get("example").unwrap(),
        )
        .unwrap();
    (source, owner, executor, count)
}

#[tokio::test]
async fn denied_http_call_never_reaches_execution() {
    let (source, owner, mut executor, count) = fixture("unattended").await;
    executor.prepare(false).await.unwrap();
    source
        .acknowledge(ToolCallResponse {
            id: "call-1".into(),
            result: Ok("not approved".into()),
        })
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn failure_after_approval_resolves_without_release_or_delivery_override() {
    let (source, owner, mut executor, count) = fixture("skip").await;
    executor.prepare(false).await.unwrap();
    executor.approve().await.unwrap();
    source
        .acknowledge(ToolCallResponse {
            id: "call-1".into(),
            result: Err("formatter failed".into()),
        })
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn declined_inquiry_finishes_without_another_attempt_or_delivery_override() {
    let (source, owner, mut executor, count) = fixture("skip").await;
    executor.prepare(false).await.unwrap();
    executor.approve().await.unwrap();
    let result = executor
        .execute(
            &IndexMap::new(),
            &Client::default(),
            "/tmp".into(),
            CancellationToken::new(),
            None,
        )
        .await;
    assert!(matches!(result, ExecutorResult::NeedsInput { .. }));
    source
        .acknowledge(ToolCallResponse {
            id: "call-1".into(),
            result: Ok("question declined".into()),
        })
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_before_release_does_not_execute() {
    let (source, owner, mut executor, count) = fixture("unattended").await;
    executor.prepare(false).await.unwrap();
    executor.approve().await.unwrap();
    let token = CancellationToken::new();
    token.cancel();
    let result = executor
        .execute(
            &IndexMap::new(),
            &Client::default(),
            "/tmp".into(),
            token,
            None,
        )
        .await;
    let ExecutorResult::Completed(response) = result else {
        panic!("expected cancelled response")
    };
    assert_eq!(response.result, Err("Tool execution cancelled.".into()));
    source.acknowledge(response).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    owner.shutdown().await.unwrap();
}

#[tokio::test]
async fn host_approval_can_wait_without_rpc_timeout() {
    let (source, owner, mut executor, count) = fixture("unattended").await;
    executor.prepare(false).await.unwrap();
    pause();
    advance(Duration::from_secs(121)).await;
    resume();
    executor.approve().await.unwrap();
    let result = executor
        .execute(
            &IndexMap::new(),
            &Client::default(),
            "/tmp".into(),
            CancellationToken::new(),
            None,
        )
        .await;
    assert!(matches!(result, ExecutorResult::NeedsInput { .. }));
    source
        .acknowledge(ToolCallResponse {
            id: "call-1".into(),
            result: Ok("declined after waiting".into()),
        })
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    owner.shutdown().await.unwrap();
}
