use std::{
    fs,
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
use camino_tempfile::tempdir;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_plugin::message::PathsInfo;
use jp_process::{ExitCode, MockProcessRunner, ProcessOutput};
use jp_tool::{Outcome, Question, ToolDefinition, ToolDocs};
use serde_json::{Value, json};
use tokio::{
    sync::{Notify, mpsc::error::TryRecvError},
    time::{Duration, timeout},
};

use super::*;
use crate::server::{
    AdmittedPlugin, PluginInit,
    builtin::BuiltinTool,
    command::sha256_file,
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
        CommandPlugins::default(),
        root.to_owned(),
        invocation,
    )
    .unwrap()
}

/// A service whose `count` tool asks one question and then echoes its input.
///
/// `run` and `result` are that tool's `run` and `result` settings, spelled as a
/// user writes them: `allow`, `ask`, `edit`, or `skip`.
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

/// What every plugin call of these tests' turns is told.
fn plugin_init() -> PluginInit {
    PluginInit {
        workspace_id: "ws-abc".to_owned(),
        storage: Some("/tmp/.jp".into()),
        paths: PathsInfo::default(),
        config: Arc::new(AppConfig::new_test()),
        log_level: 0,
    }
}

/// A service whose `count` tool is served by the command plugin `counter`.
///
/// The admission contract (`run`, Host approval, fail-closed on Host loss) is
/// the service's and does not depend on the source; these fixtures pin that for
/// a command plugin tool.
///
/// The counter records how many times the plugin's binary was run, which is the
/// only evidence that it ran: a denied call and one that silently went nowhere
/// look the same from the result alone.
fn command_fixture(run: &str, result: &str) -> (Service, HostReceiver, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let dir = tempdir().unwrap();
    let binary = dir.path().join("jp-counter");
    fs::write(&binary, "plugin").unwrap();
    let plugins = CommandPlugins::new(plugin_init()).with("counter", AdmittedPlugin {
        sha256: sha256_file(&binary).unwrap(),
        binary,
    });
    let runner = {
        let count = count.clone();
        // Owns the directory, so the binary lives as long as the service.
        MockProcessRunner::responding(move |_| {
            let _dir = &dir;
            count.fetch_add(1, Ordering::SeqCst);
            Ok(ProcessOutput {
                stdout: format!(
                    "{}\n{}\n",
                    json!({"type": "tool_outcome", "outcome": {"type": "success", "content": "ran"}}),
                    json!({"type": "exit", "code": 0}),
                ),
                stderr: String::new(),
                status: ExitCode::success(),
            })
        })
    };
    let partial: PartialToolConfig = serde_json::from_value(
        json!({"source": "plugin.command.counter", "run": run, "result": result}),
    )
    .unwrap();
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
    let (service, host) = Service::new(
        vec![tool],
        Client::default(),
        BuiltinExecutors::new(),
        Arc::new(runner),
        plugins,
        "/tmp".into(),
        InvocationContext::default(),
    )
    .unwrap();
    (service, host, count)
}

/// A service whose `count` tool is served by the command plugin `counter`,
/// configured by `config` on top of its source.
///
/// The plugin answers by the action its `init` names: `described` when asked to
/// format its arguments, `ran` when run, so a test can tell which of the two
/// produced what it sees.
/// Every run is recorded on the returned runner.
fn plugin_service(config: Value) -> (Service, HostReceiver, Arc<MockProcessRunner>) {
    let dir = tempdir().unwrap();
    let binary = dir.path().join("jp-counter");
    fs::write(&binary, "plugin").unwrap();
    let plugins = CommandPlugins::new(plugin_init()).with("counter", AdmittedPlugin {
        sha256: sha256_file(&binary).unwrap(),
        binary,
    });
    let runner = Arc::new(MockProcessRunner::responding(move |spec| {
        // Owns the directory, so the binary lives as long as the service.
        let _dir = &dir;
        let init: Value = serde_json::from_str(spec.stdin.as_deref().unwrap_or("{}")).unwrap();
        let content = match init["tool"]["action"].as_str() {
            Some("format_arguments") => "described",
            _ => "ran",
        };
        Ok(ProcessOutput {
            stdout: format!(
                "{}\n{}\n",
                json!({"type": "tool_outcome", "outcome": {"type": "success", "content": content}}),
                json!({"type": "exit", "code": 0}),
            ),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    }));

    let mut partial = json!({"source": "plugin.command.counter"});
    let Value::Object(config) = config else {
        panic!("tool config is an object");
    };
    partial.as_object_mut().unwrap().extend(config);
    let partial: PartialToolConfig = serde_json::from_value(partial).unwrap();
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
    let (service, host) = Service::new(
        vec![tool],
        Client::default(),
        BuiltinExecutors::new(),
        runner.clone(),
        plugins,
        "/tmp".into(),
        InvocationContext::default(),
    )
    .unwrap();
    (service, host, runner)
}

/// The action each run of the plugin was started for, in order.
fn actions(runner: &MockProcessRunner) -> Vec<String> {
    runner
        .calls()
        .iter()
        .map(|spec| {
            let init: Value = serde_json::from_str(spec.stdin.as_deref().unwrap()).unwrap();
            init["tool"]["action"].as_str().unwrap().to_owned()
        })
        .collect()
}

/// A plugin tool styled `parameters = "tool"` is asked to describe the call,
/// and what it answers is what the Host shows before approval.
#[tokio::test]
async fn a_plugin_describes_its_own_call_before_approval() {
    let (service, mut host, runner) = plugin_service(json!({
        "run": "ask", "result": "allow", "format": "allow", "style": {"parameters": "tool"}
    }));
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
    assert_eq!(formatted_arguments.as_deref(), Some("described"));
    assert_eq!(actions(&runner), ["format_arguments"]);

    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();

    assert_eq!(call.finish().await.unwrap(), ToolResult::text("ran"));
    assert_eq!(actions(&runner), ["format_arguments", "run"]);
}

/// `format = "ask"` holds the plugin back until the call is admitted, as it
/// does a formatter command: nothing runs before the user says yes.
#[tokio::test]
async fn a_plugin_formatter_waits_for_approval_under_format_ask() {
    let (service, mut host, runner) = plugin_service(json!({
        "run": "ask", "result": "allow", "format": "ask", "style": {"parameters": "tool"}
    }));
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
    assert_eq!(actions(&runner), Vec::<String>::new());

    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release {
        formatted_arguments,
        reply,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected release")
    };
    assert_eq!(formatted_arguments.as_deref(), Some("described"));
    assert_eq!(actions(&runner), ["format_arguments"]);
    reply
        .send(Ok(ReleaseDecision::Complete {
            result: ToolResult::text("stopped"),
        }))
        .unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    call.finish().await.unwrap();
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

/// Answers with the `path` it was given, failing for `bad`, and records the
/// order it ran in.
struct PathTool(Arc<std::sync::Mutex<Vec<String>>>);

#[async_trait]
impl BuiltinTool for PathTool {
    async fn execute(&self, arguments: &Value, _: &IndexMap<String, Value>) -> Outcome {
        let path = arguments["path"].as_str().unwrap_or_default().to_owned();
        self.0.lock().unwrap().push(path.clone());
        if path == "bad" {
            return Outcome::Error {
                message: "bad path".into(),
                trace: vec![],
                transient: false,
            };
        }
        Outcome::Success { content: path }
    }
}

/// A service whose `count` tool is a [`PathTool`] configured by `config`.
fn fan_out_fixture(config: Value) -> (Service, HostReceiver, Arc<std::sync::Mutex<Vec<String>>>) {
    let runs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (service, host) = service(
        config,
        "/tmp".into(),
        BuiltinExecutors::new().register("count", PathTool(runs.clone())),
        no_commands(),
        InvocationContext::default(),
    );
    (service, host, runs)
}

fn envelope_request(paths: &[&str]) -> CallRequest {
    let ops = paths
        .iter()
        .map(|path| json!({ "path": path }))
        .collect::<Vec<_>>();
    CallRequest {
        name: "count".into(),
        arguments: json!({ "ops": ops }).as_object().unwrap().clone(),
        correlation: Map::new(),
    }
}

/// Answer every interaction like an approving Host until the call's `Record`,
/// returning it with every `Settled` seen on the way, by operation index.
async fn approve_all(host: &mut HostReceiver) -> (Box<Recording>, Vec<(usize, Box<Recording>)>) {
    let mut settled = Vec::new();
    loop {
        let request = next(host).await;
        match request.interaction {
            Interaction::RenderArguments { reply } => reply.send(Ok(false)).unwrap(),
            Interaction::Prepare {
                arguments, reply, ..
            } => reply.send(Ok(Admission::Run { arguments })).unwrap(),
            Interaction::Release { reply, .. } => reply.send(Ok(ReleaseDecision::Execute)).unwrap(),
            Interaction::Settled { settlement, reply } => {
                let index = request
                    .call
                    .operation
                    .expect("only operations settle")
                    .index;
                settled.push((index, settlement));
                reply.send(Ok(())).unwrap();
            }
            Interaction::Record { recording, reply } => {
                assert_eq!(request.call.operation, None, "only the call records");
                reply.send(Ok(())).unwrap();
                return (recording, settled);
            }
            Interaction::Input { .. } | Interaction::Review { .. } => {
                panic!("unexpected interaction")
            }
        }
    }
}

#[tokio::test]
async fn a_fan_out_tool_is_advertised_with_the_envelope() {
    let (service, _host, _runs) = fan_out_fixture(json!({"source": "builtin", "fan_out": true}));

    let advertised = service.definitions().next().unwrap();

    assert_eq!(
        advertised.parameters,
        fan_out::envelope(&json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        }))
    );
}

#[tokio::test]
async fn a_tool_without_fan_out_is_advertised_as_declared() {
    let (service, _host, _runs) = fan_out_fixture(json!({"source": "builtin"}));

    let advertised = service.definitions().next().unwrap();

    assert_eq!(advertised.parameters["required"], json!(["path"]));
}

/// Each operation is its own invocation, talking to the Host tagged with its
/// position, and the call records once with the folded result.
#[tokio::test]
async fn each_operation_runs_as_its_own_invocation_and_the_call_records_once() {
    let (service, mut host, runs) =
        fan_out_fixture(json!({"source": "builtin", "run": "allow", "fan_out": true}));
    let call = service.start_call(envelope_request(&["a", "b"])).unwrap();

    let (recording, mut settled) = approve_all(&mut host).await;

    assert_eq!(
        recording.result,
        ToolResult::text("[1/2] ok\na\n\n[2/2] ok\nb\n")
    );
    settled.sort_by_key(|(index, _)| *index);
    assert_eq!(
        settled
            .iter()
            .map(|(index, settlement)| (*index, settlement.result.clone()))
            .collect::<Vec<_>>(),
        vec![(0, ToolResult::text("a")), (1, ToolResult::text("b"))]
    );
    assert_eq!(
        call.finish().await.unwrap(),
        ToolResult::text("[1/2] ok\na\n\n[2/2] ok\nb\n"),
        "what the caller receives is what was recorded"
    );
    assert_eq!(runs.lock().unwrap().len(), 2);
}

/// A call without the envelope is an ordinary call: no operations, no framing.
#[tokio::test]
async fn a_bare_call_to_a_fan_out_tool_runs_as_an_ordinary_call() {
    let (service, mut host, runs) =
        fan_out_fixture(json!({"source": "builtin", "run": "allow", "fan_out": true}));
    let call = service
        .start_call(CallRequest {
            name: "count".into(),
            arguments: json!({"path": "a"}).as_object().unwrap().clone(),
            correlation: Map::new(),
        })
        .unwrap();

    let (recording, settled) = approve_all(&mut host).await;

    assert!(settled.is_empty());
    assert_eq!(recording.result, ToolResult::text("a"));
    assert_eq!(call.finish().await.unwrap(), ToolResult::text("a"));
    assert_eq!(*runs.lock().unwrap(), vec!["a".to_owned()]);
}

#[tokio::test]
async fn a_malformed_envelope_is_recorded_without_running() {
    let (service, mut host, runs) =
        fan_out_fixture(json!({"source": "builtin", "run": "allow", "fan_out": true}));
    let call = service.start_call(envelope_request(&[])).unwrap();

    let (recording, _) = approve_all(&mut host).await;

    let expected = ToolResult::error(
        "Tool 'count' was called with an empty `ops` array, so there was nothing to do. Include \
         at least one operation.",
    );
    assert_eq!(recording.result, expected);
    assert_eq!(call.finish().await.unwrap(), expected);
    assert!(runs.lock().unwrap().is_empty());
}

/// Under `concurrency = 1` operations run in the order the caller wrote them,
/// even when the Host releases them in another order.
#[tokio::test]
async fn sequential_operations_run_in_the_order_written() {
    let (service, mut host, runs) = fan_out_fixture(json!({
        "source": "builtin", "run": "allow", "fan_out": {"concurrency": 1}
    }));
    let call = service
        .start_call(envelope_request(&["a", "b", "c"]))
        .unwrap();

    // Admit all three, and hold every release until all have asked for one.
    let mut releases = Vec::new();
    while releases.len() < 3 {
        let request = next(&mut host).await;
        let index = request.call.operation.unwrap().index;
        match request.interaction {
            Interaction::Prepare {
                arguments, reply, ..
            } => reply.send(Ok(Admission::Run { arguments })).unwrap(),
            Interaction::Release { reply, .. } => releases.push((index, reply)),
            _ => panic!("unexpected interaction"),
        }
    }
    releases.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
    for (_, reply) in releases {
        reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    }

    approve_all(&mut host).await;
    call.finish().await.unwrap();

    assert_eq!(*runs.lock().unwrap(), vec!["a", "b", "c"]);
}

#[tokio::test]
async fn stop_leaves_the_operations_after_a_failure_not_run() {
    let (service, mut host, runs) = fan_out_fixture(json!({
        "source": "builtin",
        "run": "allow",
        "fan_out": {"concurrency": 1, "on_error": "stop"},
    }));
    let call = service
        .start_call(envelope_request(&["bad", "good"]))
        .unwrap();

    let (recording, _) = approve_all(&mut host).await;

    assert_eq!(
        recording.result,
        ToolResult::error(
            "[1/2] error\nbad path\n\n[2/2] not run (stopped after operation 1 failed)\n"
        )
    );
    call.finish().await.unwrap();
    assert_eq!(*runs.lock().unwrap(), vec!["bad"], "the second never ran");
}

/// `result = "skip"` tells the caller the delivery was skipped whatever the
/// tool did; `stop` still acts on the tool's own failure.
#[tokio::test]
async fn stop_acts_on_a_failure_that_result_skip_hides() {
    let (service, mut host, runs) = fan_out_fixture(json!({
        "source": "builtin",
        "run": "allow",
        "result": "skip",
        "fan_out": {"concurrency": 1, "on_error": "stop"},
    }));
    let call = service
        .start_call(envelope_request(&["bad", "good"]))
        .unwrap();

    let (recording, _) = approve_all(&mut host).await;

    assert_eq!(
        recording.result,
        ToolResult::text(
            "[1/2] ok\nResult delivery skipped by configuration.\n\n[2/2] not run (stopped after \
             operation 1 failed)\n"
        )
    );
    call.finish().await.unwrap();
    assert_eq!(*runs.lock().unwrap(), vec!["bad"]);
}

/// An operation that could not be run at all (a builtin that is not registered,
/// an upstream MCP error, a command that fails to spawn) stops the rest under
/// `stop`, the same as one that ran and reported an error.
///
/// The Host holds its acknowledgement of the first operation's settlement until
/// the second has settled.
/// The gate has to learn of the failure while the first operation still holds
/// its slot: learning of it only once that settlement is acknowledged lets the
/// second start in between.
#[tokio::test]
async fn stop_acts_on_an_operation_that_could_not_run() {
    let (service, mut host) = service(
        json!({
            "source": "builtin",
            "run": "allow",
            "fan_out": {"concurrency": 1, "on_error": "stop"},
        }),
        "/tmp".into(),
        // Nothing is registered under `count`, so every attempt fails to start.
        BuiltinExecutors::new(),
        no_commands(),
        InvocationContext::default(),
    );
    let call = service.start_call(envelope_request(&["a", "b"])).unwrap();

    let mut held = None;
    let mut second_settled = false;
    let recording = loop {
        let request = next(&mut host).await;
        let index = request.call.operation.map(|operation| operation.index);
        match request.interaction {
            Interaction::RenderArguments { reply } => reply.send(Ok(false)).unwrap(),
            Interaction::Prepare {
                arguments, reply, ..
            } => reply.send(Ok(Admission::Run { arguments })).unwrap(),
            Interaction::Release { reply, .. } => reply.send(Ok(ReleaseDecision::Execute)).unwrap(),
            Interaction::Settled { reply, .. } if index == Some(0) && !second_settled => {
                held = Some(reply);
            }
            Interaction::Settled { reply, .. } => {
                reply.send(Ok(())).unwrap();
                if index == Some(1) {
                    second_settled = true;
                    if let Some(first) = held.take() {
                        first.send(Ok(())).unwrap();
                    }
                }
            }
            Interaction::Record { recording, reply } => {
                reply.send(Ok(())).unwrap();
                break recording;
            }
            Interaction::Input { .. } | Interaction::Review { .. } => {
                panic!("unexpected interaction")
            }
        }
    };

    assert_eq!(
        recording.result,
        ToolResult::error(
            "[1/2] error\nTool not found: count\n\n[2/2] not run (stopped after operation 1 \
             failed)\n"
        )
    );
    call.finish().await.unwrap();
}

/// An operation the Host declines settles without running, and the rest run.
#[tokio::test]
async fn a_declined_operation_settles_and_the_rest_run() {
    let (service, mut host, runs) =
        fan_out_fixture(json!({"source": "builtin", "run": "ask", "fan_out": true}));
    let call = service.start_call(envelope_request(&["a", "b"])).unwrap();

    let mut settled = 0;
    let recording = loop {
        let request = next(&mut host).await;
        let index = request.call.operation.map(|operation| operation.index);
        match request.interaction {
            Interaction::Prepare { reply, .. } if index == Some(0) => reply
                .send(Ok(Admission::Skip {
                    reason: "Tool skipped by user.".into(),
                }))
                .unwrap(),
            Interaction::Prepare {
                arguments, reply, ..
            } => reply.send(Ok(Admission::Run { arguments })).unwrap(),
            Interaction::Release { reply, .. } => reply.send(Ok(ReleaseDecision::Execute)).unwrap(),
            Interaction::Settled { reply, .. } => {
                settled += 1;
                reply.send(Ok(())).unwrap();
            }
            Interaction::Record { recording, reply } => {
                reply.send(Ok(())).unwrap();
                break recording;
            }
            _ => panic!("unexpected interaction"),
        }
    };

    assert_eq!(settled, 2);
    assert_eq!(
        recording.result,
        ToolResult::text("[1/2] ok\nTool skipped by user.\n\n[2/2] ok\nb\n")
    );
    call.finish().await.unwrap();
    assert_eq!(*runs.lock().unwrap(), vec!["b"]);
}

/// The Host resolving an operation itself (as it does when the user stops a
/// running tool) ends that operation with the Host's result and no settlement.
#[tokio::test]
async fn an_operation_the_host_completes_folds_the_hosts_result() {
    let (service, mut host, runs) =
        fan_out_fixture(json!({"source": "builtin", "run": "ask", "fan_out": true}));
    let call = service.start_call(envelope_request(&["a", "b"])).unwrap();

    let mut settled = 0;
    let recording = loop {
        let request = next(&mut host).await;
        let operation = request.call.operation;
        match request.interaction {
            Interaction::Prepare { .. }
                if operation.map(|operation| operation.index) == Some(0) =>
            {
                // Held open, then resolved by the Host in place of an attempt.
                assert!(service.pause_call(request.call.id));
                assert!(service.complete_call(request.call.id, ToolResult::text("cancelled")));
            }
            Interaction::Prepare {
                arguments, reply, ..
            } => reply.send(Ok(Admission::Run { arguments })).unwrap(),
            Interaction::Release { reply, .. } => reply.send(Ok(ReleaseDecision::Execute)).unwrap(),
            Interaction::Settled { reply, .. } => {
                settled += 1;
                reply.send(Ok(())).unwrap();
            }
            Interaction::Record { recording, reply } => {
                reply.send(Ok(())).unwrap();
                break recording;
            }
            _ => panic!("unexpected interaction"),
        }
    };

    assert_eq!(
        settled, 1,
        "only the operation the service concluded settles"
    );
    assert_eq!(
        recording.result,
        ToolResult::text("[1/2] ok\ncancelled\n\n[2/2] ok\nb\n")
    );
    call.finish().await.unwrap();
    assert_eq!(*runs.lock().unwrap(), vec!["b"]);
}

#[tokio::test]
async fn restart_keeps_the_logical_call_open_and_replaces_old_replies() {
    let (service, mut host, count) = fixture("ask", "allow");
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
    let (service, mut host, count) = fixture("ask", "allow");
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
async fn command_tool_waits_for_admission_before_the_plugin_runs() {
    let (service, mut host, count) = command_fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();

    let Interaction::Prepare {
        arguments, reply, ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected preparation")
    };
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "ran before it was admitted"
    );
    reply.send(Ok(Admission::Run { arguments })).unwrap();

    let Interaction::Release { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected release")
    };
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "ran before it was released"
    );
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();

    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(call.finish().await.unwrap(), ToolResult::text("ran"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn denied_command_tool_never_reaches_the_plugin() {
    let (service, mut host, count) = command_fixture("ask", "unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::Prepare { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected preparation")
    };
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
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

/// `run = "skip"` resolves the call without asking the Host and without running
/// anything.
#[tokio::test]
async fn skipped_command_tool_never_reaches_the_plugin() {
    let (service, mut host, count) = command_fixture("skip", "unattended");
    let call = service.start_call(request()).unwrap();
    let Interaction::Record { recording, reply } = next(&mut host).await.interaction else {
        panic!("expected recording, not a preparation")
    };
    assert_eq!(
        recording.result,
        ToolResult::text("Tool execution skipped by configuration.")
    );
    reply.send(Ok(())).unwrap();
    call.finish().await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_edited_arguments_never_reach_the_plugin() {
    let (service, mut host, count) = command_fixture("edit", "unattended");
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
async fn command_tool_host_loss_fails_closed() {
    let (service, host, count) = command_fixture("ask", "unattended");
    drop(host);
    let call = service.start_call(request()).unwrap();
    assert!(matches!(
        call.finish().await,
        Err(ServiceError::HostDisconnected)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_edited_arguments_do_not_reach_execution() {
    let (service, mut host, count) = fixture("edit", "allow");
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
    let (service, host, count) = fixture("ask", "allow");
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
    let (service, mut host, count) = fixture("ask", "allow");
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
    let (service, mut host, count) = fixture("ask", "allow");
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
    let (service, mut host, count) = fixture("ask", "allow");
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
    let (service, mut host, count) = fixture("ask", "allow");
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
    let (service, mut host, count) = fixture("ask", "allow");
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
    let (service, mut host, count) = fixture("skip", "allow");
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
        CommandPlugins::default(),
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
    let (service, _host, count) = fixture("ask", "allow");
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
        json!({"source": "builtin", "run": "ask", "result": "allow"}),
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
        json!({"source": "builtin", "run": "ask", "result": "allow"}),
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
    let (service, mut host, _count) = fixture("ask", "allow");
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
    let (service, mut host, count) = fixture("ask", "allow");
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

/// A service whose `count` tool is a local command styled `parameters =
/// "tool"`, so the same command both formats its arguments and runs.
///
/// The command is passed the action it was started for, and prints `described`
/// when formatting and `ran` when run, so a test can see both which runs
/// happened and which produced what it sees.
fn local_tool_formatter_fixture(format: &str) -> (Service, HostReceiver, Arc<MockProcessRunner>) {
    let runner = Arc::new(MockProcessRunner::responding(|spec| {
        let content = match spec.args.first().map(String::as_str) {
            Some("format_arguments") => "described",
            _ => "ran",
        };
        Ok(ProcessOutput {
            stdout: content.to_owned(),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    }));
    let (service, host) = service(
        json!({
            "source": "local",
            "run": "ask",
            "result": "allow",
            "format": format,
            "style": {"parameters": "tool"},
            "command": {"program": "count", "args": ["{{context.action}}"], "shell": false},
        }),
        "/tmp".into(),
        BuiltinExecutors::new(),
        runner.clone(),
        InvocationContext::default(),
    );
    (service, host, runner)
}

/// The actions the local command was started for, in order.
fn local_actions(runner: &MockProcessRunner) -> Vec<String> {
    runner
        .calls()
        .iter()
        .map(|spec| spec.args.join(" "))
        .collect()
}

/// A local tool styled `parameters = "tool"` runs its own command to describe
/// the call, and what it prints is what the Host shows before approval.
#[tokio::test]
async fn a_local_tool_describes_its_own_call_before_approval() {
    let (service, mut host, runner) = local_tool_formatter_fixture("allow");
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
    assert_eq!(formatted_arguments.as_deref(), Some("described"));
    assert_eq!(local_actions(&runner), ["format_arguments"]);

    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();

    assert_eq!(call.finish().await.unwrap(), ToolResult::text("ran"));
    assert_eq!(local_actions(&runner), ["format_arguments", "run"]);
}

/// `format = "ask"` holds the local command back until the call is admitted:
/// nothing runs before the user says yes, and a call the Host then resolves
/// itself never runs for real.
#[tokio::test]
async fn a_local_tool_formatter_waits_for_approval_under_format_ask() {
    let (service, mut host, runner) = local_tool_formatter_fixture("ask");
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
    assert_eq!(
        local_actions(&runner),
        Vec::<String>::new(),
        "nothing ran before admission"
    );

    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release {
        formatted_arguments,
        reply,
        ..
    } = next(&mut host).await.interaction
    else {
        panic!("expected release")
    };
    assert_eq!(formatted_arguments.as_deref(), Some("described"));
    reply
        .send(Ok(ReleaseDecision::Complete {
            result: ToolResult::text("stopped"),
        }))
        .unwrap();
    let Interaction::Record { reply, .. } = next(&mut host).await.interaction else {
        panic!("expected recording")
    };
    reply.send(Ok(())).unwrap();
    call.finish().await.unwrap();

    assert_eq!(local_actions(&runner), ["format_arguments"]);
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
async fn an_allowed_formatter_is_available_before_approval() {
    let (service, mut host, runner) = formatter_fixture("allow");
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
            "format": "allow",
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
    let (service, mut host, runner) = formatter_fixture("allow");
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
            "format": "allow",
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
            "format": "allow",
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
            "result": "allow",
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
    let (service, mut host, count) = asking_formatter_fixture("allow");
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
    let (service, mut host, count) = asking_formatter_fixture("allow");
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
    let (service, mut host, count) = asking_formatter_fixture("allow");
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
