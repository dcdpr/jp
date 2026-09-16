use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_conversation::event::ToolCallResponse;
use jp_mcp::server::{BuiltinTool, result::to_mcp};
use jp_tool::{
    ContentBlock, Outcome, ToolDocs, ToolResult,
    content::{ErrorDetails, Resource, ToolStatus},
};
use rmcp::model::CallToolRequestParams;
use serde_json::json;
use tokio::time::{Duration, timeout};

use super::*;

/// A tool that asks one question, then echoes the arguments and the answer.
///
/// The counter is how a test tells "the tool never ran" from "the tool ran and
/// its output went nowhere": every assertion about a denied or cancelled call
/// pairs the outcome with a count.
struct InquiringTool(Arc<AtomicUsize>);

#[async_trait]
impl BuiltinTool for InquiringTool {
    async fn execute(&self, arguments: &Value, answers: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        if let Some(answer) = answers.get("confirm") {
            return Outcome::Success {
                content: json!({"arguments": arguments, "answer": answer}).to_string(),
            };
        }
        Question::boolean("confirm", "Continue?").unwrap().into()
    }
}

struct Fixture {
    source: TerminalExecutorSource,
    owner: ExecutionOwner,
    config: ToolConfigWithDefaults,
    count: Arc<AtomicUsize>,
}

impl Fixture {
    /// Start a service exposing one `example` tool with the given config.
    async fn start(config: Value, tool: InquiringTool) -> Self {
        let partial: PartialToolConfig = serde_json::from_value(config).unwrap();
        let mut cfg = AppConfig::new_test();
        cfg.conversation.tools.insert(
            "example".into(),
            ToolConfig::from_partial(partial, vec![]).unwrap(),
        );
        let count = Arc::new(AtomicUsize::new(0));
        let definitions = vec![ToolDefinition {
            name: "example".into(),
            docs: ToolDocs::default(),
            parameters: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
            }),
        }];
        let (source, owner) = TerminalExecutorSource::start(
            BuiltinExecutors::new().register("example", tool),
            &definitions,
            &cfg.conversation.tools,
            Arc::new(ApprovalStore::default()),
            InvocationContext::default(),
            &Client::default(),
            "/tmp".into(),
        )
        .await
        .unwrap();
        Self {
            source,
            owner,
            config: cfg.conversation.tools.get("example").unwrap(),
            count,
        }
    }

    /// Start a fixture whose tool asks a question before completing.
    async fn inquiring(result_mode: &str) -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let tool = InquiringTool(count.clone());
        let mut fixture = Self::start(
            json!({"source": "builtin", "run": "ask", "result": result_mode}),
            tool,
        )
        .await;
        fixture.count = count;
        fixture
    }

    fn executor(&self, arguments: &Value) -> Box<dyn Executor> {
        self.source
            .create(
                ToolCallRequest {
                    id: "call-1".into(),
                    name: "example".into(),
                    arguments: arguments.as_object().cloned().unwrap_or_default(),
                },
                self.config.clone(),
            )
            .unwrap()
    }

    fn attempts(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Acknowledge a call, failing rather than hanging if the service never
    /// releases its barrier.
    async fn acknowledge(&self, review: Review) -> Result<(), ExecutorError> {
        timeout(Duration::from_secs(5), self.source.acknowledge(review))
            .await
            .expect("acknowledgement timed out")
    }

    async fn shutdown(self) {
        self.owner.shutdown().await.unwrap();
    }
}

fn recorded(result: Result<&str, &str>) -> Review {
    Review::replaced(ToolCallResponse {
        id: "call-1".into(),
        result: result.map(str::to_owned).map_err(str::to_owned),
    })
}

#[path = "mcp_executor_shutdown_tests.rs"]
mod shutdown;

#[tokio::test]
async fn one_call_spans_input_and_recording() {
    let fixture = Fixture::inquiring("edit").await;
    let mut executor = fixture.executor(&json!({"name": "original"}));

    assert!(executor.prepare(false).await.unwrap().is_none());
    assert_eq!(fixture.attempts(), 0);

    executor.set_arguments(json!({"name": "edited"}));
    executor.approve().await.unwrap();
    assert_eq!(fixture.attempts(), 0, "approval alone must not execute");

    let first = executor
        .execute(&IndexMap::new(), CancellationToken::new(), None)
        .await;
    let ExecutorResult::NeedsInput { question, .. } = first else {
        panic!("expected the tool's question, got {first:?}")
    };
    assert_eq!(question, Question::boolean("confirm", "Continue?").unwrap());
    assert_eq!(fixture.attempts(), 1);

    let second = executor
        .execute(
            &IndexMap::from_iter([("confirm".into(), json!(true))]),
            CancellationToken::new(),
            None,
        )
        .await;
    let ExecutorResult::Completed(response) = second else {
        panic!("expected a completed call, got {second:?}")
    };
    // The answer reached a second execution of the same logical call, and the
    // arguments it ran with are the edited ones.
    assert_eq!(
        response.result,
        Ok(r#"{"arguments":{"name":"edited"},"answer":true}"#.into())
    );
    assert_eq!(fixture.attempts(), 2);

    fixture.acknowledge(recorded(Ok("reviewed"))).await.unwrap();
    assert_eq!(fixture.attempts(), 2, "acknowledgement must not re-execute");
    fixture.shutdown().await;
}

/// A result carrying everything the conversation's text projection drops.
fn rich_result() -> ToolResult {
    ToolResult {
        content: vec![
            ContentBlock::text("plain text"),
            ContentBlock::Resource(Resource::text("file:///a", "embedded")),
        ],
        status: ToolStatus::Error(ErrorDetails {
            transient: true,
            trace: vec!["upstream".into()],
        }),
        structured_content: Some(json!({"answer": 42})),
        metadata: None,
    }
}

#[test]
fn an_unedited_review_delivers_the_result_the_service_offered() {
    let offered = rich_result();
    let recorded = response("call-1", &offered);
    // The conversation keeps only the text, and it is an error, so a result
    // rebuilt from it would be a single text block with no resource, no
    // structured content, and default error details.
    assert_eq!(
        recorded.result,
        Err("plain text\n\nembedded".into()),
        "the text projection is what the conversation records"
    );

    let delivered = approved(Some(offered.clone()), &Review::unchanged(recorded));

    assert_eq!(delivered, offered);
}

#[test]
fn an_edited_review_delivers_the_content_the_host_recorded() {
    let offered = rich_result();
    let edited = Review::replaced(ToolCallResponse {
        id: "call-1".into(),
        result: Ok("the user rewrote this".into()),
    });

    let delivered = approved(Some(offered), &edited);

    // Editing replaces the content outright: the caller must not receive the
    // resource, structured content, or error status of a result the Host chose
    // not to deliver.
    assert_eq!(delivered, ToolResult::text("the user rewrote this"));
}

#[test]
fn a_barrier_with_no_result_behind_it_delivers_the_recorded_content() {
    // A call the Host denied before execution has no result of its own, so
    // there is nothing to preserve and the recorded text is all there is.
    let denied = Review::unchanged(ToolCallResponse {
        id: "call-1".into(),
        result: Err("not approved".into()),
    });

    assert_eq!(approved(None, &denied), ToolResult::error("not approved"));
}

#[tokio::test]
async fn an_unedited_review_reaches_the_service_through_a_real_call() {
    // The unit tests above pin the decision; this pins that a review actually
    // reaches it, rather than the call resolving at some earlier barrier.
    let fixture = Fixture::inquiring("ask").await;
    let mut executor = fixture.executor(&json!({}));
    assert!(executor.prepare(false).await.unwrap().is_none());
    executor.approve().await.unwrap();

    let first = executor
        .execute(&IndexMap::new(), CancellationToken::new(), None)
        .await;
    assert!(matches!(first, ExecutorResult::NeedsInput { .. }));

    let second = executor
        .execute(
            &IndexMap::from_iter([("confirm".into(), json!(true))]),
            CancellationToken::new(),
            None,
        )
        .await;
    let ExecutorResult::Completed(response) = second else {
        panic!("expected a reviewable result, got {second:?}")
    };

    // Recording the offered content unchanged completes the call: the service
    // accepts its own result back and returns it to the caller.
    fixture
        .acknowledge(Review::unchanged(response))
        .await
        .unwrap();
    assert_eq!(fixture.attempts(), 2);
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_denied_call_completes_without_executing() {
    let fixture = Fixture::inquiring("unattended").await;
    let mut executor = fixture.executor(&json!({}));
    assert!(executor.prepare(false).await.unwrap().is_none());

    fixture
        .acknowledge(recorded(Ok("not approved")))
        .await
        .unwrap();

    assert_eq!(fixture.attempts(), 0, "a denied call must not run the tool");
    // Acknowledging again is a no-op rather than an error: the call is gone.
    fixture
        .acknowledge(recorded(Ok("not approved")))
        .await
        .unwrap();
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_failure_after_approval_resolves_the_call() {
    let fixture = Fixture::inquiring("skip").await;
    let mut executor = fixture.executor(&json!({}));
    executor.prepare(false).await.unwrap();
    executor.approve().await.unwrap();

    // The Host abandons the call at the release barrier rather than executing.
    fixture
        .acknowledge(recorded(Err("formatter failed")))
        .await
        .unwrap();

    assert_eq!(fixture.attempts(), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_declined_inquiry_finishes_without_another_attempt() {
    let fixture = Fixture::inquiring("skip").await;
    let mut executor = fixture.executor(&json!({}));
    executor.prepare(false).await.unwrap();
    executor.approve().await.unwrap();

    let result = executor
        .execute(&IndexMap::new(), CancellationToken::new(), None)
        .await;
    assert!(matches!(result, ExecutorResult::NeedsInput { .. }));
    assert_eq!(fixture.attempts(), 1);

    fixture
        .acknowledge(recorded(Ok("question declined")))
        .await
        .unwrap();

    assert_eq!(
        fixture.attempts(),
        1,
        "declining the question must not run the tool again"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn cancellation_before_release_does_not_execute() {
    let fixture = Fixture::inquiring("unattended").await;
    let mut executor = fixture.executor(&json!({}));
    executor.prepare(false).await.unwrap();
    executor.approve().await.unwrap();

    let token = CancellationToken::new();
    token.cancel();
    let result = executor.execute(&IndexMap::new(), token, None).await;

    let ExecutorResult::Completed(response) = result else {
        panic!("expected a cancelled response, got {result:?}")
    };
    assert_eq!(response.result, Err("Tool execution cancelled.".into()));
    assert_eq!(fixture.attempts(), 0);

    fixture
        .acknowledge(Review::unchanged(response))
        .await
        .unwrap();
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_protocol_failure_is_reported_as_a_failure_not_as_tool_output() {
    // Executing before the call is released puts the adapter and the service
    // out of step. That is JP's problem, so it must not arrive as a tool
    // result the model reads as "the tool said this".
    let fixture = Fixture::inquiring("unattended").await;
    let executor = fixture.executor(&json!({}));

    let result = executor
        .execute(&IndexMap::new(), CancellationToken::new(), None)
        .await;

    let ExecutorResult::Failed(error) = result else {
        panic!("a protocol failure must not be a tool result, got {result:?}")
    };
    assert_eq!(
        error.to_string(),
        "MCP call cannot execute while not yet submitted"
    );
    assert_eq!(fixture.attempts(), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn preparing_a_call_twice_is_refused() {
    let fixture = Fixture::inquiring("unattended").await;
    let mut executor = fixture.executor(&json!({}));
    executor.prepare(false).await.unwrap();

    let error = executor.prepare(false).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "MCP call cannot be submitted while awaiting admission"
    );
    assert_eq!(fixture.attempts(), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn approval_is_refused_before_the_call_is_submitted() {
    let fixture = Fixture::inquiring("unattended").await;
    let mut executor = fixture.executor(&json!({}));

    let error = executor.approve().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "MCP call cannot be approved while not awaiting admission"
    );
    assert_eq!(fixture.attempts(), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn caller_metadata_cannot_claim_another_call() {
    // The correlation key is the Host's own, so an MCP call that arrives
    // without it (or with the wrong one) never reaches a Host route and fails
    // closed rather than borrowing another call's approval.
    let fixture = Fixture::inquiring("unattended").await;
    let _executor = fixture.executor(&json!({}));

    let mut params = CallToolRequestParams::new("example");
    params.arguments = Some(Map::new());
    params.meta = Some(Meta(
        json!({"computer.jp/hostCall": "0".repeat(32)})
            .as_object()
            .unwrap()
            .clone(),
    ));
    let peer = fixture.source.peer.clone();
    let call = tokio::spawn(async move { peer.call_tool(params).await });

    // No Host route accepts the forged key, so the service's interaction is
    // dropped and the call ends without an admission decision.
    let outcome = timeout(Duration::from_secs(5), call)
        .await
        .expect("forged call must not hang")
        .unwrap();
    assert!(
        outcome.is_err(),
        "a forged correlation key must not execute"
    );
    assert_eq!(fixture.attempts(), 0);
    fixture.shutdown().await;
}

#[test]
fn a_rich_result_projects_to_text_and_survives_the_mcp_round_trip() {
    // Pins the compatibility projection the conversation stores against the
    // result the caller receives: the first drops everything but text, the
    // second keeps all of it.
    let result = ToolResult {
        content: vec![
            ContentBlock::text("plain text"),
            ContentBlock::Resource(Resource::text("file:///a", "embedded")),
        ],
        status: ToolStatus::Success,
        structured_content: Some(json!({"answer": 42})),
        metadata: None,
    };

    assert_eq!(
        response("call-1", &result).result,
        Ok("plain text\n\nembedded".into())
    );
    assert_eq!(
        serde_json::to_value(to_mcp(result).unwrap()).unwrap(),
        json!({
            "content": [
                {"type": "text", "text": "plain text"},
                {"type": "resource", "resource": {"uri": "file:///a", "text": "embedded"}},
            ],
            "structuredContent": {"answer": 42},
            "isError": false,
        })
    );
}
