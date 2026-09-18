use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
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
use crate::cmd::query::tool::executor::mock::no_commands;

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

/// A tool that answers with its `name` argument, so each call's result says
/// which arguments it ran with.
struct EchoName(Arc<AtomicUsize>);

#[async_trait]
impl BuiltinTool for EchoName {
    async fn execute(&self, arguments: &Value, _: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        Outcome::Success {
            content: arguments["name"].as_str().unwrap_or_default().to_owned(),
        }
    }
}

/// A tool that runs until its attempt is abandoned, so an interrupt always
/// lands while it is still in flight.
struct BlockingTool(Arc<AtomicUsize>);

#[async_trait]
impl BuiltinTool for BlockingTool {
    async fn execute(&self, _: &Value, _: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        pending().await
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
    async fn start(config: Value, tool: impl BuiltinTool + 'static) -> Self {
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
            no_commands(),
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
            .operations
            .pop()
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

/// Start a fixture whose `example` tool echoes its `name` and fans out.
async fn fanning_out(run: &str) -> Fixture {
    let count = Arc::new(AtomicUsize::new(0));
    let mut fixture = Fixture::start(
        json!({"source": "builtin", "run": run, "fan_out": true}),
        EchoName(count.clone()),
    )
    .await;
    fixture.count = count;
    fixture
}

/// A call to `example` carrying one operation per name.
fn envelope(names: &[&str]) -> ToolCallRequest {
    let ops = names
        .iter()
        .map(|name| json!({ "name": name }))
        .collect::<Vec<_>>();
    ToolCallRequest {
        id: "call-1".into(),
        name: "example".into(),
        arguments: json!({ "ops": ops }).as_object().unwrap().clone(),
    }
}

/// Run an approved operation to its result.
async fn run(executor: &dyn Executor) -> ToolCallResponse {
    let prepared = executor.prepare(false, CancellationToken::new()).await;
    assert!(
        matches!(prepared, ExecutorResult::AwaitingAdmission),
        "expected admission, got {prepared:?}"
    );
    let approved = executor.approve(CancellationToken::new()).await;
    assert!(
        matches!(approved, ExecutorResult::AwaitingRelease),
        "expected release, got {approved:?}"
    );
    let result = executor
        .execute(&IndexMap::new(), CancellationToken::new(), None)
        .await;
    let ExecutorResult::Completed(response) = result else {
        panic!("expected a completed operation, got {result:?}")
    };
    response
}

/// The skip message an operation the user declined settles with.
fn skipped() -> Review {
    Review::unchanged(ToolCallResponse {
        id: "call-1".into(),
        result: Ok("Tool skipped by user.".into()),
    })
}

/// Wait for a fanned-out call's folded response, failing rather than hanging.
async fn folded(fan_out: &dyn FanOutCall) -> ToolCallResponse {
    timeout(Duration::from_secs(5), fan_out.recorded())
        .await
        .expect("the call is recorded")
        .unwrap()
}

/// A call carrying the envelope becomes one executor per operation, each
/// running its own arguments, and the call records the folded result, which is
/// also what its MCP request delivers.
#[tokio::test]
async fn a_fanned_out_call_runs_each_operation_and_records_the_fold() {
    let fixture = fanning_out("allow").await;

    let CallExecutors {
        operations,
        fan_out,
    } = fixture
        .source
        .create(envelope(&["first", "second"]), fixture.config.clone())
        .unwrap();
    let fan_out = fan_out.expect("the call fans out");
    assert_eq!(
        operations
            .iter()
            .map(|executor| executor.state_key())
            .collect::<Vec<_>>(),
        vec!["call-1#0", "call-1#1"]
    );

    let mut responses = Vec::new();
    for executor in &operations {
        responses.push(run(executor.as_ref()).await);
    }
    assert_eq!(
        responses
            .iter()
            .map(|response| response.result.clone())
            .collect::<Vec<_>>(),
        vec![Ok("first".to_owned()), Ok("second".to_owned())]
    );
    for (executor, response) in operations.iter().zip(responses) {
        executor.settle(Review::unchanged(response)).await.unwrap();
    }

    let folded = folded(fan_out.as_ref()).await;
    assert_eq!(folded, ToolCallResponse {
        id: "call-1".into(),
        result: Ok("[1/2] ok\nfirst\n\n[2/2] ok\nsecond\n".into()),
    });

    // Acknowledgement checks the MCP response against what was recorded.
    fixture
        .acknowledge(Review::unchanged(folded))
        .await
        .unwrap();
    assert_eq!(fixture.attempts(), 2, "each operation ran once");
    fixture.shutdown().await;
}

/// A call to a fan-out tool without the envelope is an ordinary call.
#[tokio::test]
async fn a_bare_call_to_a_fan_out_tool_has_one_executor() {
    let fixture = fanning_out("allow").await;

    let prepared = fixture
        .source
        .create(
            ToolCallRequest {
                id: "call-1".into(),
                name: "example".into(),
                arguments: json!({"name": "only"}).as_object().unwrap().clone(),
            },
            fixture.config.clone(),
        )
        .unwrap();
    assert!(prepared.fan_out.is_none());
    let executor = prepared.operations.into_iter().next().unwrap();
    assert_eq!(executor.state_key(), "call-1");

    let response = run(executor.as_ref()).await;
    assert_eq!(response.result, Ok("only".into()));
    fixture
        .acknowledge(Review::unchanged(response))
        .await
        .unwrap();
    fixture.shutdown().await;
}

/// An operation the user declines is resolved with its skip message, which is
/// folded into the call's result, and the tool never runs for it.
#[tokio::test]
async fn a_declined_operation_is_folded_with_its_decision() {
    let fixture = fanning_out("ask").await;

    let CallExecutors {
        operations,
        fan_out,
    } = fixture
        .source
        .create(envelope(&["first", "second"]), fixture.config.clone())
        .unwrap();
    let [declined, approved] = operations.try_into().ok().unwrap();

    let prepared = declined.prepare(false, CancellationToken::new()).await;
    assert!(matches!(prepared, ExecutorResult::AwaitingAdmission));
    declined.settle(skipped()).await.unwrap();
    let response = run(approved.as_ref()).await;
    approved.settle(Review::unchanged(response)).await.unwrap();

    let folded = folded(fan_out.unwrap().as_ref()).await;
    assert_eq!(
        folded.result,
        Ok("[1/2] ok\nTool skipped by user.\n\n[2/2] ok\nsecond\n".into())
    );
    fixture
        .acknowledge(Review::unchanged(folded))
        .await
        .unwrap();
    assert_eq!(fixture.attempts(), 1, "the declined operation never ran");
    fixture.shutdown().await;
}

/// Operations settled before any of them was prepared still fold.
///
/// The Host settles a call the moment a "no" remembered for its tool reaches
/// it, which can be before its first step has sent anything.
/// The service only starts the operations once their call's MCP request
/// arrives, so settling one has to send that request; otherwise nothing ever
/// answers, and the call is never recorded.
#[tokio::test]
async fn operations_settled_before_any_is_prepared_still_fold() {
    let fixture = fanning_out("ask").await;

    let CallExecutors {
        operations,
        fan_out,
    } = fixture
        .source
        .create(envelope(&["first", "second"]), fixture.config.clone())
        .unwrap();
    for executor in &operations {
        timeout(Duration::from_secs(5), executor.settle(skipped()))
            .await
            .expect("settling does not hang")
            .unwrap();
    }

    let folded = folded(fan_out.unwrap().as_ref()).await;
    assert_eq!(
        folded.result,
        Ok("[1/2] ok\nTool skipped by user.\n\n[2/2] ok\nTool skipped by user.\n".into())
    );
    fixture
        .acknowledge(Review::unchanged(folded))
        .await
        .unwrap();
    assert_eq!(fixture.attempts(), 0, "no operation ran");
    fixture.shutdown().await;
}

#[tokio::test]
async fn one_call_spans_input_and_recording() {
    let fixture = Fixture::inquiring("edit").await;
    let executor = fixture.executor(&json!({"name": "original"}));

    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert_eq!(fixture.attempts(), 0);

    executor.set_arguments(json!({"name": "edited"}));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));
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
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));

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
    let fixture = Fixture::inquiring("allow").await;
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));

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
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));

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
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));

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
    let fixture = Fixture::inquiring("allow").await;
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));

    let token = CancellationToken::new();
    token.cancel();
    let result = executor.execute(&IndexMap::new(), token, None).await;

    let ExecutorResult::Completed(response) = result else {
        panic!("expected a cancelled response, got {result:?}")
    };
    assert_eq!(fixture.attempts(), 0);

    fixture
        .acknowledge(Review::unchanged(response))
        .await
        .unwrap();
    fixture.shutdown().await;
}

/// Swap the reply the service is parked on for one the test holds, and return
/// its receiving end.
///
/// What the Host sends on it can then be read back exactly, rather than
/// inferred from whether the service raced to act on it.
/// The service's own reply is dropped, so it ends the call on its side.
async fn intercept<T>(
    fixture: &Fixture,
    swap: impl FnOnce(Phase, Reply<T>) -> Phase,
) -> oneshot::Receiver<HostReply<T>> {
    let slot = locked(&fixture.source.calls).by_id[&("call-1".to_owned(), None)].clone();
    let mut state = slot.state.lock().await;
    let (sender, receiver) = oneshot::channel();
    let parked = mem::replace(&mut state.phase, Phase::Finished);
    state.phase = swap(parked, sender);
    receiver
}

/// A release step cancelled before it ran sends nothing.
///
/// The service would start the tool as soon as the release reached it, on
/// another thread, before a cancellation noticed afterwards could stop it.
#[tokio::test]
async fn a_release_cancelled_before_it_runs_is_not_sent() {
    let fixture = Fixture::inquiring("allow").await;
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));
    let mut sent = intercept(&fixture, |parked, reply| {
        assert!(matches!(parked, Phase::Release(_)), "{}", parked.name());
        Phase::Release(reply)
    })
    .await;

    let token = CancellationToken::new();
    token.cancel();
    let result = executor.execute(&IndexMap::new(), token, None).await;

    let ExecutorResult::Completed(response) = result else {
        panic!("expected a cancelled response, got {result:?}")
    };
    assert_eq!(response.result, Err("Tool execution cancelled.".into()));
    assert!(
        matches!(sent.try_recv(), Err(oneshot::error::TryRecvError::Closed)),
        "the cancelled step released the call"
    );
    fixture.shutdown().await;
}

/// An answer step cancelled before it ran does not send the answer, which would
/// run the tool again.
#[tokio::test]
async fn an_answer_cancelled_before_it_runs_is_not_sent() {
    let fixture = Fixture::inquiring("allow").await;
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));
    let first = executor
        .execute(&IndexMap::new(), CancellationToken::new(), None)
        .await;
    assert!(matches!(first, ExecutorResult::NeedsInput { .. }));
    let mut sent = intercept(&fixture, |parked, reply| {
        let Phase::Input { id, .. } = parked else {
            panic!(
                "expected the call parked on its question, got {}",
                parked.name()
            )
        };
        Phase::Input { id, reply }
    })
    .await;

    let token = CancellationToken::new();
    token.cancel();
    let answers = IndexMap::from_iter([("confirm".into(), json!(true))]);
    let result = executor.execute(&answers, token, None).await;

    let ExecutorResult::Completed(response) = result else {
        panic!("expected a cancelled response, got {result:?}")
    };
    assert_eq!(response.result, Err("Tool execution cancelled.".into()));
    assert!(
        matches!(sent.try_recv(), Err(oneshot::error::TryRecvError::Closed)),
        "the cancelled step sent the answer"
    );
    assert_eq!(fixture.attempts(), 1);
    fixture.shutdown().await;
}

/// An approval step cancelled before it ran does not admit the call, which
/// would start a formatter held back until admission.
#[tokio::test]
async fn an_admission_cancelled_before_it_runs_is_not_sent() {
    let fixture = Fixture::inquiring("allow").await;
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    let mut sent = intercept(&fixture, |parked, reply| {
        assert!(matches!(parked, Phase::Admission(_)), "{}", parked.name());
        Phase::Admission(reply)
    })
    .await;

    let token = CancellationToken::new();
    token.cancel();
    let result = executor.approve(token).await;

    let ExecutorResult::Completed(response) = result else {
        panic!("expected a cancelled response, got {result:?}")
    };
    assert_eq!(response.result, Err("Tool execution cancelled.".into()));
    assert!(
        matches!(sent.try_recv(), Err(oneshot::error::TryRecvError::Closed)),
        "the cancelled step admitted the call"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_held_call_delivers_the_recorded_response_to_its_agent() {
    // An agent owns the MCP request and builds its own transcript from the
    // response, so the text the Host recorded for a cancelled call is only seen
    // by the model if it is what the agent receives.
    let count = Arc::new(AtomicUsize::new(0));
    let mut fixture = Fixture::start(
        json!({"source": "builtin", "run": "allow"}),
        BlockingTool(count.clone()),
    )
    .await;
    fixture.count = count;
    fixture
        .source
        .set_execution(ToolExecution::Agent {
            correlation_key: "test/agentId",
        })
        .unwrap();
    let executor = fixture.executor(&json!({}));

    let mut params = CallToolRequestParams::new("example");
    params.arguments = Some(Map::new());
    params.meta = Some(Meta(Map::from_iter([(
        "test/agentId".into(),
        "call-1".into(),
    )])));
    let peer = fixture.source.peer.clone();
    let agent = tokio::spawn(async move { peer.call_tool(params).await });

    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));

    let token = CancellationToken::new();
    let answers = IndexMap::new();
    let running = executor.execute(&answers, token.clone(), None);
    tokio::pin!(running);
    let started = async {
        while fixture.attempts() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    timeout(Duration::from_secs(5), async {
        tokio::select! {
            result = &mut running => panic!("the tool must still be running, got {result:?}"),
            () = started => {}
        }
    })
    .await
    .expect("the tool never started");

    assert!(executor.hold_for_response(), "a named call can be held");
    token.cancel();
    let result = running.await;
    assert!(
        matches!(result, ExecutorResult::Completed(_)),
        "a held call reports its attempt as over, got {result:?}"
    );

    fixture
        .acknowledge(recorded(Ok(
            "Tool run cancelled by user with a custom message:\n\nuse grep instead"
        )))
        .await
        .unwrap();

    let delivered = timeout(Duration::from_secs(5), agent)
        .await
        .expect("the agent's call must finish")
        .unwrap()
        .map(|result| serde_json::to_value(result).unwrap())
        .map_err(|error| error.to_string());
    assert_eq!(
        delivered,
        Ok(json!({
            "content": [{
                "type": "text",
                "text": "Tool run cancelled by user with a custom message:\n\nuse grep instead",
            }],
            "isError": false,
        }))
    );
    assert_eq!(fixture.attempts(), 1, "holding must not run the tool again");
    fixture.shutdown().await;
}

/// A call settled before its agent submits it, as when a "no" remembered for
/// its tool lands first, answers the agent's request with the recorded response
/// once that request arrives.
#[tokio::test]
async fn a_call_held_before_its_agent_submits_it_delivers_the_recorded_response() {
    let fixture = Fixture::inquiring("allow").await;
    fixture
        .source
        .set_execution(ToolExecution::Agent {
            correlation_key: "test/agentId",
        })
        .unwrap();
    let executor = fixture.executor(&json!({}));

    // Nothing has arrived for the call, so preparing it waits for the agent.
    let token = CancellationToken::new();
    let preparing = executor.prepare(false, token.clone());
    tokio::pin!(preparing);
    assert!(
        timeout(Duration::from_millis(50), &mut preparing)
            .await
            .is_err(),
        "preparation finished before the agent submitted the call"
    );

    assert!(executor.hold_for_response(), "an unnamed call can be held");
    token.cancel();
    let result = preparing.await;
    assert!(
        matches!(result, ExecutorResult::Completed(_)),
        "a held call reports its attempt as over, got {result:?}"
    );

    let mut params = CallToolRequestParams::new("example");
    params.arguments = Some(Map::new());
    params.meta = Some(Meta(Map::from_iter([(
        "test/agentId".into(),
        "call-1".into(),
    )])));
    let peer = fixture.source.peer.clone();
    // Submitted after the acknowledgement has started, the order an agent's
    // request takes when JP settles the call as soon as it is announced.
    let agent = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        timeout(Duration::from_secs(5), peer.call_tool(params)).await
    };
    let (acknowledged, delivered) = tokio::join!(
        fixture.acknowledge(recorded(Ok("Tool skipped by user (remembered)."))),
        agent,
    );
    acknowledged.unwrap();

    let delivered = delivered
        .expect("the agent's call must finish")
        .map(|result| serde_json::to_value(result).unwrap())
        .map_err(|error| error.to_string());
    assert_eq!(
        delivered,
        Ok(json!({
            "content": [{"type": "text", "text": "Tool skipped by user (remembered)."}],
            "isError": false,
        }))
    );
    assert_eq!(fixture.attempts(), 0, "a settled call must not run");
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_protocol_failure_is_reported_as_a_failure_not_as_tool_output() {
    // Executing before the call is released puts the adapter and the service
    // out of step. That is JP's problem, so it must not arrive as a tool
    // result the model reads as "the tool said this".
    let fixture = Fixture::inquiring("allow").await;
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

/// A tool that does its work and then never reports back.
struct VanishingTool(Arc<AtomicUsize>);

#[async_trait]
impl BuiltinTool for VanishingTool {
    async fn execute(&self, _: &Value, _: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("the side effect happened; the result is lost");
    }
}

#[tokio::test]
async fn a_call_lost_after_release_is_not_reported_as_unexecuted() {
    // The tool has already acted by the time the call fails, so telling the
    // model it did not run would invite it to repeat the side effect.
    let count = Arc::new(AtomicUsize::new(0));
    let mut fixture = Fixture::start(
        json!({"source": "builtin", "run": "allow"}),
        VanishingTool(count.clone()),
    )
    .await;
    fixture.count = count;
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));
    assert!(matches!(
        executor.approve(CancellationToken::new()).await,
        ExecutorResult::AwaitingRelease
    ));

    let result = timeout(
        Duration::from_secs(5),
        executor.execute(&IndexMap::new(), CancellationToken::new(), None),
    )
    .await
    .expect("a lost call must still end");

    assert!(
        matches!(result, ExecutorResult::OutcomeUnknown(_)),
        "a call lost after release may have run, got {result:?}"
    );
    assert_eq!(fixture.attempts(), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn preparing_a_call_twice_is_refused() {
    let fixture = Fixture::inquiring("allow").await;
    let executor = fixture.executor(&json!({}));
    assert!(matches!(
        executor.prepare(false, CancellationToken::new()).await,
        ExecutorResult::AwaitingAdmission
    ));

    let ExecutorResult::Failed(error) = executor.prepare(false, CancellationToken::new()).await
    else {
        panic!("expected a refusal")
    };
    assert_eq!(
        error.to_string(),
        "MCP call cannot be submitted while awaiting admission"
    );
    assert_eq!(fixture.attempts(), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn approval_is_refused_before_the_call_is_submitted() {
    let fixture = Fixture::inquiring("allow").await;
    let executor = fixture.executor(&json!({}));

    let ExecutorResult::Failed(error) = executor.approve(CancellationToken::new()).await else {
        panic!("expected a refusal")
    };
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
    let fixture = Fixture::inquiring("allow").await;
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
