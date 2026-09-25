use std::{
    future::pending,
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use assert_matches::assert_matches;
use async_trait::async_trait;
use camino::Utf8Path;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_process::{ExitCode, MockProcessRunner, ProcessOutput};
use jp_tool::{Outcome, Question, ToolDefinition, ToolDocs};
use serde_json::{Value, json};
use tokio::{
    sync::{Notify, mpsc::error::TryRecvError},
    time::{Duration, timeout},
};

use super::*;
use crate::server::{
    builtin::BuiltinTool,
    testing::{echoing, no_commands, printed},
};

struct CountingTool(Arc<AtomicUsize>);

#[async_trait]
impl BuiltinTool for CountingTool {
    async fn execute(&self, arguments: &Value, answers: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        match answers.get("confirm") {
            Some(answer) => Outcome::Success {
                content: json!({"arguments": arguments, "answer": answer}).to_string(),
            },
            None => Question::boolean("confirm", "Proceed?")
                .unwrap()
                .with_preamble("Review this operation.")
                .into(),
        }
    }
}

/// Build a service around one builtin tool named `count`.
///
/// `config` is the tool's configuration as a user would write it, so a test
/// says what it needs rather than patching a service after construction.
fn service(
    config: Value,
    root: &Utf8Path,
    builtins: BuiltinExecutors,
    runner: Arc<dyn ProcessRunner>,
    invocation: InvocationContext,
) -> (Service, HostReceiver) {
    let partial: PartialToolConfig = serde_json::from_value(config).unwrap();
    let mut app = AppConfig::new_test();
    app.conversation.tools.insert(
        "count".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let tool = ConfiguredTool {
        definition: ToolDefinition {
            name: "count".into(),
            docs: ToolDocs::default(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
            }),
        },
        config: app.conversation.tools.get("count").unwrap(),
        access: Ok(None),
        metadata: Map::new(),
    };
    Service::new(
        vec![tool],
        Client::default(),
        builtins,
        runner,
        root.to_owned(),
        invocation,
    )
    .unwrap()
}

/// A service whose `count` tool asks one question and then echoes its input.
///
/// `run` and `result` are that tool's `run` and `result` settings, spelled as a
/// user writes them: `unattended`, `ask`, `edit`, or `skip`.
///
/// The counter records how many execution attempts actually ran, which is what
/// separates "the call was denied" from "the call silently went nowhere".
fn fixture(run: &str, result: &str) -> (Service, HostReceiver, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let (service, host) = service(
        json!({"source": "builtin", "run": run, "result": result}),
        "/tmp".into(),
        BuiltinExecutors::new().register("count", CountingTool(count.clone())),
        no_commands(),
        InvocationContext::default(),
    );
    (service, host, count)
}

async fn release(host: &mut HostReceiver) {
    let Interaction::Prepare {
        arguments, reply, ..
    } = next(host).await.interaction
    else {
        panic!("expected preparation")
    };
    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release { reply, .. } = next(host).await.interaction else {
        panic!("expected release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
}

async fn next(host: &mut HostReceiver) -> HostRequest {
    timeout(Duration::from_secs(2), host.recv())
        .await
        .unwrap()
        .unwrap()
}

fn request() -> CallRequest {
    CallRequest {
        name: "count".into(),
        arguments: json!({"path":"original"}).as_object().unwrap().clone(),
        correlation: Map::new(),
    }
}

#[tokio::test]
async fn preparation_release_input_and_delivery_use_distinct_acknowledgements() {
    let (service, mut host, count) = fixture("edit", "edit");
    let call = service.start_call(request()).unwrap();
    let id = call.id();
    let prepared = next(&mut host).await;
    assert_eq!(prepared.call.id, id);
    let Interaction::Prepare {
        arguments, reply, ..
    } = prepared.interaction
    else {
        panic!("expected preparation")
    };
    assert_eq!(arguments, request().arguments);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    reply
        .send(Ok(Admission::Run {
            arguments: json!({"path":"edited"}).as_object().unwrap().clone(),
        }))
        .unwrap();
    let Interaction::Release {
        arguments, reply, ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected release")
    };
    assert_eq!(arguments["path"], "edited");
    assert_eq!(count.load(Ordering::SeqCst), 0);
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let Interaction::Input {
        request,
        supporting,
        answers,
        reply,
    } = next(&mut host).await.interaction
    else {
        panic!("expected input")
    };
    assert_eq!(request.id.as_str(), "confirm");
    assert_eq!(supporting, vec![ContentBlock::text(
        "Review this operation."
    )]);
    assert!(answers.is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    reply.send(Ok(json!(true).into())).unwrap();
    let Interaction::Review { result, reply, .. } = next(&mut host).await.interaction else {
        panic!("expected review")
    };
    assert_eq!(
        result,
        ToolResult::text(r#"{"arguments":{"path":"edited"},"answer":true}"#)
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    reply.send(Ok(ToolResult::text("edited result"))).unwrap();
    let Interaction::Record { recording, reply } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    assert_eq!(recording.result, ToolResult::text("edited result"));
    assert!(!call.is_finished());
    reply.send(Ok(())).unwrap();
    assert_eq!(
        call.finish().await.unwrap(),
        ToolResult::text("edited result")
    );
    service.shutdown().await;
}

#[tokio::test]
async fn restart_keeps_the_logical_call_open_and_replaces_old_replies() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    let id = call.id();
    release(&mut host).await;
    let old = next(&mut host).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(service.pause_call(id));
    service.resume_call(id);
    release(&mut host).await;
    let Interaction::Input { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected input")
    };
    assert!(old.interaction.is_expired());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(!call.is_finished());
    reply.send(Ok(InputAnswer::Answer(json!(true)))).unwrap();
    let recorded = next(&mut host).await;
    assert_eq!(recorded.call.id, id);
    let Interaction::Record { reply, .. } = recorded.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        call.finish().await.unwrap(),
        ToolResult::text(r#"{"arguments":{"path":"original"},"answer":true}"#)
    );
    assert_eq!(count.load(Ordering::SeqCst), 3);
    service.shutdown().await;
}

#[tokio::test]
async fn denied_call_never_executes() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::Prepare { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected preparation")
    };
    reply
        .send(Ok(Admission::Skip {
            reason: "denied".into(),
        }))
        .unwrap();
    let Interaction::Record { recording, reply } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    assert_eq!(recording.result, ToolResult::text("denied"));
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), ToolResult::text("denied"));
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_edited_arguments_do_not_reach_execution() {
    let (service, mut host, count) = fixture("edit", "unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::Prepare { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected preparation")
    };
    reply
        .send(Ok(Admission::Run {
            arguments: Map::new(),
        }))
        .unwrap();
    assert!(matches!(
        call.finish().await,
        Err(ServiceError::Tool(ToolError::Arguments { .. }))
    ));
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn host_loss_fails_closed() {
    let (service, host, count) = fixture("ask", "unattended");
    drop(host);
    let call = service.start_call(request()).unwrap();
    assert!(matches!(
        call.finish().await,
        Err(ServiceError::HostDisconnected)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn shutdown_cancels_pending_release_and_rejects_late_reply() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::Prepare { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected preparation")
    };
    reply
        .send(Ok(Admission::Run {
            arguments: request().arguments,
        }))
        .unwrap();
    let Interaction::Release { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected release")
    };
    timeout(Duration::from_secs(2), service.shutdown())
        .await
        .unwrap();
    assert!(reply.send(Ok(ReleaseDecision::Execute)).is_err());
    assert!(matches!(call.finish().await, Err(ServiceError::Cancelled)));
    assert!(matches!(
        service.start_call(request()),
        Err(ServiceError::Stopped)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_answer_prevents_a_second_attempt() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    release(&mut host).await;
    let Interaction::Input { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected input")
    };
    assert_eq!(count.load(Ordering::SeqCst), 1);
    reply.send(Ok(json!("not a boolean").into())).unwrap();
    assert!(matches!(call.finish().await, Err(ServiceError::InvalidAnswer(id)) if id == "confirm"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_recording_prevents_result_delivery() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    release(&mut host).await;
    let Interaction::Input { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected input")
    };
    reply.send(Ok(json!(true).into())).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply
        .send(Err(HostError::Recording(Arc::new(io::Error::other(
            "disk full",
        )))))
        .unwrap();
    assert_matches!(
        call.finish().await,
        Err(ServiceError::Host(HostError::Recording(source)))
            if source.to_string() == "disk full"
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn current_call_cancellation_does_not_poison_later_calls() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let first = service.start_call(request()).unwrap();
    release(&mut host).await;
    let Interaction::Input { reply: stale, .. } = next(&mut host).await.interaction else {
        panic!("expected input")
    };
    service.cancel_current();
    assert!(matches!(first.finish().await, Err(ServiceError::Cancelled)));
    assert!(stale.send(Ok(json!(true).into())).is_err());
    let second = service.start_call(request()).unwrap();
    release(&mut host).await;
    let Interaction::Input { answers, reply, .. } = next(&mut host).await.interaction else {
        panic!("expected fresh input")
    };
    assert!(answers.is_empty());
    reply.send(Ok(json!(false).into())).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        second.finish().await.unwrap(),
        ToolResult::text(r#"{"arguments":{"path":"original"},"answer":false}"#)
    );
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn identical_calls_have_independent_answers_and_out_of_order_delivery() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let first = service.start_call(request()).unwrap();
    release(&mut host).await;
    let first_input = next(&mut host).await;
    assert_eq!(first_input.call.id, first.id());
    let Interaction::Input {
        reply: first_answer,
        ..
    } = first_input.interaction
    else {
        panic!("expected first input")
    };
    let second = service.start_call(request()).unwrap();
    assert_ne!(first.id(), second.id());
    release(&mut host).await;
    let second_input = next(&mut host).await;
    assert_eq!(second_input.call.id, second.id());
    let Interaction::Input {
        reply: second_answer,
        answers,
        ..
    } = second_input.interaction
    else {
        panic!("expected second input")
    };
    assert!(answers.is_empty());
    second_answer.send(Ok(json!(false).into())).unwrap();
    let record = next(&mut host).await;
    assert_eq!(record.call.id, second.id());
    let Interaction::Record { reply, .. } = record.interaction else {
        panic!("expected second recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        second.finish().await.unwrap(),
        ToolResult::text(r#"{"arguments":{"path":"original"},"answer":false}"#)
    );
    assert!(!first.is_finished());
    first_answer.send(Ok(json!(true).into())).unwrap();
    let record = next(&mut host).await;
    assert_eq!(record.call.id, first.id());
    let Interaction::Record { reply, .. } = record.interaction else {
        panic!("expected first recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        first.finish().await.unwrap(),
        ToolResult::text(r#"{"arguments":{"path":"original"},"answer":true}"#)
    );
    assert_eq!(count.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn configured_skip_never_requests_execution_release() {
    let (service, mut host, count) = fixture("skip", "unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::Record { recording, reply } = next(&mut host).await.interaction else {
        panic!("skip must go directly to recording")
    };
    assert_eq!(recording.raw_result, None);
    reply.send(Ok(())).unwrap();
    assert_eq!(
        call.finish().await.unwrap(),
        ToolResult::text("Tool execution skipped by configuration.")
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn skipped_delivery_records_original_without_delivering_it() {
    let (service, mut host, count) = fixture("ask", "skip");
    let call = service.start_call(request()).unwrap();
    release(&mut host).await;
    let Interaction::Input { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected input")
    };
    reply.send(Ok(json!(true).into())).unwrap();
    let Interaction::Record { recording, reply } = next(&mut host).await.interaction else {
        panic!("expected recording, no review")
    };
    // The tool's own output is recorded even though the user never sees it.
    assert_eq!(
        recording.raw_result,
        Some(ToolResult::text(
            r#"{"arguments":{"path":"original"},"answer":true}"#
        ))
    );
    assert_eq!(
        recording.result,
        ToolResult::text("Result delivery skipped by configuration.")
    );
    reply.send(Ok(())).unwrap();
    assert_eq!(
        call.finish().await.unwrap(),
        ToolResult::text("Result delivery skipped by configuration.")
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn local_inquiry_exits_and_runs_a_new_process_with_the_answer() {
    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source":"local", "run":"ask",
        "command": {"program":"probe", "args":["{{tool.answers.confirm | default('null')}}"], "shell":false}
    })).unwrap();
    // Asks until it is given an answer, then prints the answer.
    let asking = serde_json::to_string(&Outcome::NeedsInput {
        question: Question::boolean("confirm", "Proceed?").unwrap(),
    })
    .unwrap();
    let runner = Arc::new(MockProcessRunner::responding(move |spec| {
        Ok(printed(match spec.args.as_slice() {
            [answer] if answer != "null" => answer.clone(),
            _ => asking.clone(),
        }))
    }));
    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "local".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let tool = ConfiguredTool {
        definition: ToolDefinition {
            name: "local".into(),
            docs: ToolDocs::default(),
            parameters: json!({"type":"object","properties":{}}),
        },
        config: cfg.conversation.tools.get("local").unwrap(),
        access: Ok(None),
        metadata: Map::new(),
    };
    let (service, mut host) = Service::new(
        vec![tool],
        Client::default(),
        BuiltinExecutors::new(),
        runner.clone(),
        "/tmp".into(),
        InvocationContext::default(),
    )
    .unwrap();
    let call = service
        .start_call(CallRequest {
            name: "local".into(),
            arguments: Map::new(),
            correlation: Map::new(),
        })
        .unwrap();
    release(&mut host).await;
    let Interaction::Input { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected local input")
    };
    let attempts = |runner: &MockProcessRunner| -> Vec<Vec<String>> {
        runner.calls().into_iter().map(|spec| spec.args).collect()
    };
    assert_eq!(attempts(&runner), [["null"]]);
    reply.send(Ok(json!(true).into())).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    assert_eq!(attempts(&runner), [["null"], ["true"]]);
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), ToolResult::text("true"));
}

#[tokio::test]
async fn wrong_argument_type_fails_before_host_approval() {
    let (service, _host, count) = fixture("ask", "unattended");
    let mut input = request();
    input.arguments.insert("path".into(), json!(42));
    let call = service.start_call(input).unwrap();
    let finished = timeout(Duration::from_secs(2), call.finish())
        .await
        .unwrap();
    assert_matches!(
        finished,
        Err(ServiceError::InvalidArgument { path }) if path == "path"
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

struct BlockedTool {
    entered: Arc<Notify>,
    dropped: Arc<AtomicUsize>,
}
struct RunningAttempt(Arc<AtomicUsize>);
impl Drop for RunningAttempt {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl BuiltinTool for BlockedTool {
    async fn execute(&self, _: &Value, _: &IndexMap<String, Value>) -> Outcome {
        let _attempt = RunningAttempt(self.dropped.clone());
        self.entered.notify_one();
        pending().await
    }
}

#[tokio::test]
async fn cancellation_drops_an_in_flight_builtin_attempt() {
    let entered = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    let (service, mut host) = service(
        json!({"source": "builtin", "run": "ask", "result": "unattended"}),
        "/tmp".into(),
        BuiltinExecutors::new().register("count", BlockedTool {
            entered: entered.clone(),
            dropped: dropped.clone(),
        }),
        no_commands(),
        InvocationContext::default(),
    );
    let call = service.start_call(request()).unwrap();
    release(&mut host).await;
    timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    call.cancel();
    assert!(matches!(call.finish().await, Err(ServiceError::Cancelled)));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_completed_call_delivers_the_host_result_in_place_of_its_attempt() {
    let entered = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    let (service, mut host) = service(
        json!({"source": "builtin", "run": "ask", "result": "unattended"}),
        "/tmp".into(),
        BuiltinExecutors::new().register("count", BlockedTool {
            entered: entered.clone(),
            dropped: dropped.clone(),
        }),
        no_commands(),
        InvocationContext::default(),
    );
    let call = service.start_call(request()).unwrap();
    let id = call.id();
    release(&mut host).await;
    timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();

    assert!(service.complete_call(id, ToolResult::text("stopped by the user")));

    let finished = timeout(Duration::from_secs(2), call.finish())
        .await
        .expect("a completed call must finish");
    assert_matches!(finished, Ok(result) if result == ToolResult::text("stopped by the user"));
    assert_eq!(dropped.load(Ordering::SeqCst), 1, "the attempt must stop");
    // The Host recorded the result before completing the call, so it is not
    // asked to record or review it again.
    assert!(matches!(host.try_recv(), Err(TryRecvError::Empty)));
    service.shutdown().await;
}

#[tokio::test]
async fn completing_a_finished_call_is_refused() {
    let (service, mut host, _count) = fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    let id = call.id();
    let Interaction::Prepare { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected preparation")
    };
    reply
        .send(Ok(Admission::Complete {
            result: ToolResult::text("denied"),
        }))
        .unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), ToolResult::text("denied"));
    // The result is sent before the call leaves the active set.
    timeout(Duration::from_secs(2), async {
        while service.call_cancellation(id).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    assert!(!service.complete_call(id, ToolResult::text("too late")));
    service.shutdown().await;
}

#[tokio::test]
async fn dropping_result_receiver_does_not_cancel_or_reexecute() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    release(&mut host).await;
    let Interaction::Input { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected input")
    };
    drop(call);
    reply.send(Ok(json!(true).into())).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    service.shutdown().await;
}

/// A service whose `count` tool formats its arguments with a `formatter`
/// command, and the runner that plays it.
///
/// The formatter echoes the action and the invocation identity the service
/// supplied it, and the runner records each run, so a test can tell "the
/// formatter did not run" from "it ran and produced nothing".
fn formatter_fixture(mode: &str) -> (Service, HostReceiver, Arc<MockProcessRunner>) {
    let runner = echoing();
    let (service, host) = service(
        json!({
            "source": "builtin",
            "run": "ask",
            "format": mode,
            "style": {"parameters": {
                "program": "formatter",
                "args": [
                    "{{context.action}}:{{tool.arguments.path}}:{{context.workspace_id}}/{{context.conversation_id}}",
                ],
                "shell": false,
            }},
        }),
        "/tmp".into(),
        BuiltinExecutors::new().register("count", CountingTool(Arc::new(AtomicUsize::new(0)))),
        runner.clone(),
        InvocationContext {
            workspace_id: "ws-abc".into(),
            conversation_id: "conv-xyz".into(),
        },
    );
    (service, host, runner)
}

#[tokio::test]
async fn formatter_asks_for_visibility_and_waits_for_approval() {
    let (service, mut host, runner) = formatter_fixture("ask");
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();
    let Interaction::Prepare {
        reply,
        formatted_arguments,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected approval")
    };
    assert!(formatted_arguments.is_none());
    assert_eq!(runner.calls(), vec![]);
    reply
        .send(Ok(Admission::Run {
            arguments: request().arguments,
        }))
        .unwrap();
    let Interaction::Release {
        reply,
        formatted_arguments,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected release")
    };
    // The formatter runs under the action, arguments, and invocation identity
    // the service supplies, not values a caller could set.
    assert_eq!(
        formatted_arguments.as_deref(),
        Some("format_arguments:original:ws-abc/conv-xyz")
    );
    assert_eq!(runner.calls().len(), 1);
    call.cancel();
    assert!(matches!(call.finish().await, Err(ServiceError::Cancelled)));
    assert!(reply.send(Ok(ReleaseDecision::Execute)).is_err());
}

#[tokio::test]
async fn unattended_formatter_is_available_before_approval() {
    let (service, mut host, runner) = formatter_fixture("unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();
    let Interaction::Prepare {
        formatted_arguments,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected preparation")
    };
    assert_eq!(
        formatted_arguments.as_deref(),
        Some("format_arguments:original:ws-abc/conv-xyz")
    );
    assert_eq!(runner.calls().len(), 1);
    call.cancel();
    assert!(matches!(call.finish().await, Err(ServiceError::Cancelled)));
}

#[tokio::test]
async fn a_formatter_is_told_the_name_the_tool_runs_under() {
    // A `source` naming an implementation (`builtin.counter` under the key
    // `count`) is the name the tool executes as, so the formatter is asked
    // about that name rather than the key the assistant called. Handing it the
    // key asks about a tool that does not exist.
    let (service, mut host) = service(
        json!({
            "source": "builtin.counter",
            "run": "ask",
            "format": "unattended",
            "style": {"parameters": {
                "program": "formatter",
                "args": ["{{tool.name}}"],
                "shell": false,
            }},
        }),
        "/tmp".into(),
        BuiltinExecutors::new().register("counter", CountingTool(Arc::new(AtomicUsize::new(0)))),
        echoing(),
        InvocationContext::default(),
    );
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();
    let Interaction::Prepare {
        formatted_arguments,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected preparation")
    };
    assert_eq!(formatted_arguments.as_deref(), Some("counter"));
    call.cancel();
    assert!(matches!(call.finish().await, Err(ServiceError::Cancelled)));
}

#[tokio::test]
async fn hidden_presentation_never_executes_formatter() {
    let (service, mut host, runner) = formatter_fixture("unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(false)).unwrap();
    let Interaction::Prepare {
        formatted_arguments,
        reply,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected preparation")
    };
    assert!(formatted_arguments.is_none());
    reply
        .send(Ok(Admission::Skip {
            reason: "denied".into(),
        }))
        .unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), ToolResult::text("denied"));
    assert_eq!(runner.calls(), vec![]);
}

/// A formatter that fails settles the call with its error: nobody could see
/// what the call would do, so it is not put up for approval and never runs.
#[tokio::test]
async fn a_failing_formatter_settles_the_call_without_running_it() {
    let count = Arc::new(AtomicUsize::new(0));
    let (service, mut host) = service(
        json!({
            "source": "builtin",
            "run": "ask",
            "format": "unattended",
            "style": {"parameters": {"program": "formatter", "args": [], "shell": false}},
        }),
        "/tmp".into(),
        BuiltinExecutors::new().register("count", CountingTool(count.clone())),
        Arc::new(
            MockProcessRunner::builder()
                .expect("formatter")
                .returns(ProcessOutput {
                    stdout: String::new(),
                    stderr: "no preview".into(),
                    status: ExitCode::from_code(3),
                }),
        ),
        InvocationContext::default(),
    );
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();

    let Interaction::Record { recording, reply } = next(&mut host).await.interaction else {
        panic!("expected recording, not preparation")
    };
    let failure = ToolResult::error(
        "Tool 'count' was not executed because the argument formatter failed: {\"message\":\"Tool \
         'count' execution failed.\",\"stderr\":\"no preview\",\"stdout\":\"\"}",
    );
    assert_eq!(recording.result, failure);
    assert_eq!(recording.raw_result, None);
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), failure);
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

/// A failure the formatter reports itself reads as its message and trace, not
/// as the object a run's failure is reported to the model in.
#[tokio::test]
async fn a_formatters_reported_failure_settles_the_call_with_its_message() {
    let reported = serde_json::to_string(&Outcome::Error {
        message: "The shorter title is 71 characters.".into(),
        trace: vec!["limit is 60".into()],
        transient: true,
    })
    .unwrap();
    let (service, mut host) = service(
        json!({
            "source": "builtin",
            "run": "ask",
            "format": "unattended",
            "style": {"parameters": {"program": "formatter", "args": [], "shell": false}},
        }),
        "/tmp".into(),
        BuiltinExecutors::new().register("count", CountingTool(Arc::default())),
        Arc::new(
            MockProcessRunner::builder()
                .expect("formatter")
                .returns_success(reported),
        ),
        InvocationContext::default(),
    );
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();

    let Interaction::Record { recording, reply } = next(&mut host).await.interaction else {
        panic!("expected recording, not preparation")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        recording.result,
        ToolResult::error(
            "Tool 'count' was not executed because the argument formatter failed: The shorter \
             title is 71 characters.\n\nTrace:\nlimit is 60"
        )
    );
    call.finish().await.unwrap();
}

/// A service whose `count` tool has a formatter that asks the tool's `confirm`
/// question, and describes the call with the answer once it has one.
///
/// `format` is the tool's `format` setting.
/// The counter records how many times the tool itself ran.
fn asking_formatter_fixture(format: &str) -> (Service, HostReceiver, Arc<AtomicUsize>) {
    // Serialized rather than hand-written, so the formatter speaks the wire
    // format a real tool emits.
    let question = serde_json::to_string(&Outcome::NeedsInput {
        question: Question::boolean("confirm", "Proceed?").unwrap(),
    })
    .unwrap();
    // An unanswered question renders as `null`.
    let runner = MockProcessRunner::responding(move |spec| {
        Ok(printed(match spec.args.as_slice() {
            [answer] if answer != "null" => format!("confirmed:{answer}"),
            _ => question.clone(),
        }))
    });
    let count = Arc::new(AtomicUsize::new(0));
    let (service, host) = service(
        json!({
            "source": "builtin",
            "run": "ask",
            "result": "unattended",
            "format": format,
            "style": {"parameters": {
                "program": "formatter",
                "args": ["{{tool.answers.confirm}}"],
                "shell": false,
            }},
        }),
        "/tmp".into(),
        BuiltinExecutors::new().register("count", CountingTool(count.clone())),
        Arc::new(runner),
        InvocationContext::default(),
    );
    (service, host, count)
}

/// Take the next interaction, which must be the formatter's `confirm` question.
async fn formatter_question(host: &mut HostReceiver) -> oneshot::Sender<HostReply<InputAnswer>> {
    let Interaction::Input { request, reply, .. } = next(host).await.interaction else {
        panic!("expected the formatter's question")
    };
    assert_eq!(request.id.as_str(), "confirm");
    reply
}

/// Record the call and return what the caller received.
async fn record(host: &mut HostReceiver, call: Call) -> ToolResult {
    let Interaction::Record { reply, .. } = next(host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    call.finish().await.unwrap()
}

/// The formatter's question is answered before the call is put up for approval,
/// so the Host approves the call as the formatter describes it with the answer.
/// The tool runs once, with that same answer, and asks nothing.
#[tokio::test]
async fn a_formatters_question_is_answered_before_the_call_is_prepared() {
    let (service, mut host, count) = asking_formatter_fixture("unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();
    formatter_question(&mut host)
        .await
        .send(Ok(json!(true).into()))
        .unwrap();

    let Interaction::Prepare {
        arguments,
        formatted_arguments,
        reply,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected preparation")
    };
    assert_eq!(formatted_arguments.as_deref(), Some("confirmed:true"));
    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release {
        formatted_arguments,
        reply,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected release")
    };
    assert_eq!(formatted_arguments.as_deref(), Some("confirmed:true"));
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();

    // Recording follows release directly: the tool already had its answer.
    assert_eq!(
        record(&mut host, call).await,
        ToolResult::text(r#"{"arguments":{"path":"original"},"answer":true}"#)
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// A formatter held back until admission asks its question between admission
/// and release, so the call is still described before anything runs.
#[tokio::test]
async fn a_formatter_held_until_admission_asks_before_release() {
    let (service, mut host, count) = asking_formatter_fixture("ask");
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();
    let Interaction::Prepare {
        arguments,
        formatted_arguments,
        reply,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected preparation")
    };
    assert!(formatted_arguments.is_none());
    reply.send(Ok(Admission::Run { arguments })).unwrap();
    formatter_question(&mut host)
        .await
        .send(Ok(json!(true).into()))
        .unwrap();

    let Interaction::Release {
        formatted_arguments,
        reply,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected release")
    };
    assert_eq!(formatted_arguments.as_deref(), Some("confirmed:true"));
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    assert_eq!(
        record(&mut host, call).await,
        ToolResult::text(r#"{"arguments":{"path":"original"},"answer":true}"#)
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// A Host that resolves the call instead of answering the formatter settles it
/// there: nothing is prepared, and the tool never runs.
#[tokio::test]
async fn a_settled_formatter_question_records_the_call_without_running_it() {
    let (service, mut host, count) = asking_formatter_fixture("unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();
    formatter_question(&mut host)
        .await
        .send(Ok(InputAnswer::Complete {
            result: ToolResult::text("declined"),
        }))
        .unwrap();

    assert_eq!(record(&mut host, call).await, ToolResult::text("declined"));
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

/// An answer given about the original arguments is not carried over to
/// arguments the Host edited: the formatter asks again, and the tool runs with
/// the new answer.
#[tokio::test]
async fn edited_arguments_ask_the_formatters_question_again() {
    let (service, mut host, count) = asking_formatter_fixture("unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::RenderArguments { reply } = next(&mut host).await.interaction else {
        panic!("expected visibility request")
    };
    reply.send(Ok(true)).unwrap();
    formatter_question(&mut host)
        .await
        .send(Ok(json!(true).into()))
        .unwrap();
    let Interaction::Prepare { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected preparation")
    };
    reply
        .send(Ok(Admission::Run {
            arguments: json!({"path": "edited"}).as_object().unwrap().clone(),
        }))
        .unwrap();

    formatter_question(&mut host)
        .await
        .send(Ok(json!(false).into()))
        .unwrap();
    let Interaction::Release {
        formatted_arguments,
        reply,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected release")
    };
    assert_eq!(formatted_arguments.as_deref(), Some("confirmed:false"));
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    assert_eq!(
        record(&mut host, call).await,
        ToolResult::text(r#"{"arguments":{"path":"edited"},"answer":false}"#)
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
