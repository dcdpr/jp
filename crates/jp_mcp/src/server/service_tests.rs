#[cfg(unix)]
use std::fs;
use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
#[cfg(unix)]
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_tool::{Outcome, Question, ToolDefinition, ToolDocs};
use serde_json::{Value, json};
use tokio::{
    sync::Notify,
    time::{Duration, timeout},
};

use super::*;
use crate::server::builtin::BuiltinTool;

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

fn fixture(run: &str, result: &str) -> (Service, HostReceiver, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let partial: PartialToolConfig =
        serde_json::from_value(json!({"source":"builtin", "run":run, "result":result})).unwrap();
    let mut config = AppConfig::new_test();
    config.conversation.tools.insert(
        "count".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let tool = ConfiguredTool {
        definition: ToolDefinition {
            name: "count".into(),
            docs: ToolDocs::default(),
            parameters: json!({"type":"object", "properties":{"path":{"type":"string"}}, "required":["path"]}),
        },
        config: config.conversation.tools.get("count").unwrap(),
        access: None,
    };
    let (service, host) = Service::new(
        vec![tool],
        Client::default(),
        BuiltinExecutors::new().register("count", CountingTool(count.clone())),
        "/tmp".into(),
        InvocationContext::default(),
    )
    .unwrap();
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
    reply.send(Ok(())).unwrap();
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
    reply.send(Ok(())).unwrap();
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
    reply.send(Ok(json!(true))).unwrap();
    let Interaction::Review { result, reply, .. } = next(&mut host).await.interaction else {
        panic!("expected review")
    };
    assert_eq!(
        result,
        Ok(r#"{"arguments":{"path":"edited"},"answer":true}"#.into())
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    reply.send(Ok(Ok("edited result".into()))).unwrap();
    let Interaction::Record { result, reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    assert_eq!(result, Ok("edited result".into()));
    assert!(!call.is_finished());
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), Ok("edited result".into()));
    service.shutdown().await.unwrap();
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
    let Interaction::Record { reply, result, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    assert_eq!(result, Ok("denied".into()));
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), Ok("denied".into()));
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
        .unwrap()
        .unwrap();
    assert!(reply.send(Ok(())).is_err());
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
    reply.send(Ok(json!("not a boolean"))).unwrap();
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
    reply.send(Ok(json!(true))).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Err(HostError("disk full".into()))).unwrap();
    assert!(
        matches!(call.finish().await, Err(ServiceError::Host(HostError(reason))) if reason == "disk full")
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
    assert!(stale.send(Ok(json!(true))).is_err());
    let second = service.start_call(request()).unwrap();
    release(&mut host).await;
    let Interaction::Input { answers, reply, .. } = next(&mut host).await.interaction else {
        panic!("expected fresh input")
    };
    assert!(answers.is_empty());
    reply.send(Ok(json!(false))).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        second.finish().await.unwrap(),
        Ok(r#"{"arguments":{"path":"original"},"answer":false}"#.into())
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
    second_answer.send(Ok(json!(false))).unwrap();
    let record = next(&mut host).await;
    assert_eq!(record.call.id, second.id());
    let Interaction::Record { reply, .. } = record.interaction else {
        panic!("expected second recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        second.finish().await.unwrap(),
        Ok(r#"{"arguments":{"path":"original"},"answer":false}"#.into())
    );
    assert!(!first.is_finished());
    first_answer.send(Ok(json!(true))).unwrap();
    let record = next(&mut host).await;
    assert_eq!(record.call.id, first.id());
    let Interaction::Record { reply, .. } = record.interaction else {
        panic!("expected first recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        first.finish().await.unwrap(),
        Ok(r#"{"arguments":{"path":"original"},"answer":true}"#.into())
    );
    assert_eq!(count.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn configured_skip_never_requests_execution_release() {
    let (service, mut host, count) = fixture("skip", "unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::Record {
        reply, raw_result, ..
    } = next(&mut host).await.interaction
    else {
        panic!("skip must go directly to recording")
    };
    assert_eq!(raw_result, None);
    reply.send(Ok(())).unwrap();
    assert_eq!(
        call.finish().await.unwrap(),
        Ok("Tool execution skipped by configuration.".into())
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
    reply.send(Ok(json!(true))).unwrap();
    let Interaction::Record {
        reply,
        raw_result,
        result,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected recording, no review")
    };
    assert_eq!(
        raw_result,
        Some(Ok(
            r#"{"arguments":{"path":"original"},"answer":true}"#.into()
        ))
    );
    assert_eq!(
        result,
        Ok("Result delivery skipped by configuration.".into())
    );
    reply.send(Ok(())).unwrap();
    assert_eq!(
        call.finish().await.unwrap(),
        Ok("Result delivery skipped by configuration.".into())
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
#[cfg(unix)]
async fn local_inquiry_exits_and_runs_a_new_process_with_the_answer() {
    let root = tempdir().unwrap();
    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source":"local", "run":"ask",
        "command": {"program":"sh", "args":["-c", "printf 'run\\n' >> attempts; if [ \"$1\" = null ]; then printf '%s' '{\"type\":\"needs_input\",\"question\":{\"id\":\"confirm\",\"text\":\"Proceed?\",\"answer_type\":{\"type\":\"boolean\"},\"pre_amble\":null,\"default\":null}}'; else printf '%s' \"$1\"; fi", "probe", "{{tool.answers.confirm | default('null')}}"], "shell":false}
    })).unwrap();
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
        access: None,
    };
    let (service, mut host) = Service::new(
        vec![tool],
        Client::default(),
        BuiltinExecutors::new(),
        root.path().to_owned(),
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
    assert_eq!(
        fs::read_to_string(root.path().join("attempts")).unwrap(),
        "run\n"
    );
    reply.send(Ok(json!(true))).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    assert_eq!(
        fs::read_to_string(root.path().join("attempts")).unwrap(),
        "run\nrun\n"
    );
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), Ok("true".into()));
}

#[tokio::test]
async fn wrong_argument_type_fails_before_host_approval() {
    let (service, _host, count) = fixture("ask", "unattended");
    let mut input = request();
    input.arguments.insert("path".into(), json!(42));
    let call = service.start_call(input).unwrap();
    assert!(
        matches!(timeout(Duration::from_secs(2), call.finish()).await.unwrap(), Err(ServiceError::InvalidArgument { path }) if path == "path")
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
    let (mut service, mut host, _) = fixture("ask", "unattended");
    let entered = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    Arc::get_mut(&mut service.inner).unwrap().builtins =
        BuiltinExecutors::new().register("count", BlockedTool {
            entered: entered.clone(),
            dropped: dropped.clone(),
        });
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
async fn dropping_result_receiver_does_not_cancel_or_reexecute() {
    let (service, mut host, count) = fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    release(&mut host).await;
    let Interaction::Input { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected input")
    };
    drop(call);
    reply.send(Ok(json!(true))).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    service.shutdown().await.unwrap();
}

#[cfg(unix)]
fn formatter_fixture(mode: &str) -> (Service, HostReceiver, Utf8TempDir) {
    let (mut service, host, _) = fixture("ask", "unattended");
    let root = tempdir().unwrap();
    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source":"builtin", "run":"ask", "format":mode,
        "style":{"parameters":{"program":"sh", "args":["-c", "printf 'formatted' > formatter-ran; printf '%s' '{{context.action}}:{{tool.arguments.path}}'"], "shell":false}}
    })).unwrap();
    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "count".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let inner = Arc::get_mut(&mut service.inner).unwrap();
    inner.root = root.path().to_owned();
    inner.tools.get_mut("count").unwrap().config = cfg.conversation.tools.get("count").unwrap();
    (service, host, root)
}

#[tokio::test]
#[cfg(unix)]
async fn formatter_asks_for_visibility_and_waits_for_approval() {
    let (service, mut host, root) = formatter_fixture("ask");
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
    assert_eq!(formatted_arguments, None);
    assert!(!root.path().join("formatter-ran").exists());
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
    assert_eq!(
        formatted_arguments,
        Some(Ok("format_arguments:original".into()))
    );
    assert_eq!(
        fs::read_to_string(root.path().join("formatter-ran")).unwrap(),
        "formatted"
    );
    call.cancel();
    assert!(matches!(call.finish().await, Err(ServiceError::Cancelled)));
    assert!(reply.send(Ok(())).is_err());
}

#[tokio::test]
#[cfg(unix)]
async fn unattended_formatter_is_available_before_approval() {
    let (service, mut host, root) = formatter_fixture("unattended");
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
        formatted_arguments,
        Some(Ok("format_arguments:original".into()))
    );
    assert!(root.path().join("formatter-ran").exists());
    call.cancel();
    assert!(matches!(call.finish().await, Err(ServiceError::Cancelled)));
}

#[tokio::test]
#[cfg(unix)]
async fn hidden_presentation_never_executes_formatter() {
    let (service, mut host, root) = formatter_fixture("unattended");
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
    assert_eq!(formatted_arguments, None);
    reply
        .send(Ok(Admission::Skip {
            reason: "denied".into(),
        }))
        .unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), Ok("denied".into()));
    assert!(!root.path().join("formatter-ran").exists());
}
