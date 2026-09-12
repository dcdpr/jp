//! Independent HTTP client fixtures for the MCP Host/third-party boundary.

#[cfg(unix)]
use std::fs;
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use camino_tempfile::{Utf8TempDir, tempdir};
use indexmap::IndexMap;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_tool::{Outcome, Question, ToolResult};
use reqwest::{Client as HttpClient, Response, redirect::Policy};
use rmcp::model::{CallToolRequestParams, Meta};
use serde_json::{Map, Value, json};
use tokio::{
    sync::mpsc::error::TryRecvError,
    time::{Duration, timeout},
};

use super::Endpoint;
use crate::{
    Client, Content,
    server::{
        InvocationContext,
        builtin::{BuiltinExecutors, BuiltinTool},
        service::{
            Admission, ConfiguredTool, HostError, HostReceiver, HostRequest, InputAnswer,
            Interaction, ReleaseDecision, Service,
        },
        tool_definitions,
    },
};

const PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Clone)]
struct ExternalClient {
    http: HttpClient,
    url: String,
    session: String,
}

impl ExternalClient {
    async fn connect(url: &str) -> Self {
        let http = HttpClient::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let response = http.post(url).header("accept", "application/json, text/event-stream").json(&json!({
            "jsonrpc":"2.0", "id":0, "method":"initialize",
            "params":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"third-party-fixture","version":"1"}}
        })).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        let session = response
            .headers()
            .get("mcp-session-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let initialized = SseReader::new(response).reply(0).await;
        assert_eq!(initialized["result"]["protocolVersion"], PROTOCOL_VERSION);
        let client = Self {
            http,
            url: url.into(),
            session,
        };
        client.notify("notifications/initialized", json!({})).await;
        client
    }

    async fn request(&self, id: u64, method: &str, params: Value) -> SseReader {
        let response = self
            .http
            .post(&self.url)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
        SseReader::new(response)
    }

    async fn notify(&self, method: &str, params: Value) {
        let response = self
            .http
            .post(&self.url)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .json(&json!({"jsonrpc":"2.0","method":method,"params":params}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 202);
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"");
    }

    async fn resume(&self, event_id: &str) -> SseReader {
        let response = self
            .http
            .get(&self.url)
            .header("accept", "text/event-stream")
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .header("last-event-id", event_id)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
        SseReader::new(response)
    }

    async fn close(self) {
        let response = self
            .http
            .delete(&self.url)
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 202);
    }
}

struct SseReader {
    response: Response,
    buffer: Vec<u8>,
}

impl SseReader {
    fn new(response: Response) -> Self {
        Self {
            response,
            buffer: Vec::new(),
        }
    }

    // Parse complete LF-framed events, including priming events with no data.
    // Decoding after finding the delimiter handles split UTF-8 code points.
    async fn frame(&mut self) -> (Option<String>, Option<Value>) {
        loop {
            if let Some(offset) = self.buffer.windows(2).position(|bytes| bytes == b"\n\n") {
                let bytes = self.buffer.drain(..offset + 2).collect::<Vec<_>>();
                let frame = String::from_utf8(bytes).unwrap();
                let id = frame
                    .lines()
                    .find_map(|line| line.strip_prefix("id:"))
                    .map(|value| value.trim().to_owned());
                let data = frame
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                return (
                    id,
                    (!data.is_empty()).then(|| serde_json::from_str(&data).unwrap()),
                );
            }
            let chunk = self
                .response
                .chunk()
                .await
                .unwrap()
                .expect("SSE ended before the response");
            self.buffer.extend_from_slice(&chunk);
        }
    }

    async fn reply(mut self, id: u64) -> Value {
        loop {
            if let (_, Some(message)) = self.frame().await {
                assert_eq!(message["jsonrpc"], "2.0");
                assert_eq!(message["id"], id);
                return message;
            }
        }
    }
}

struct Fixture {
    endpoint: Endpoint,
    host: HostReceiver,
    root: Utf8TempDir,
}

impl Fixture {
    async fn shutdown(self) {
        self.endpoint.shutdown().await.unwrap();
        // The working directory must outlive service cleanup.
        drop(self.root);
    }
}

async fn fixture(config: Value, builtins: BuiltinExecutors) -> Fixture {
    let root = tempdir().unwrap();
    let mut cfg = AppConfig::new_test();
    let config: PartialToolConfig = serde_json::from_value(config).unwrap();
    cfg.conversation.tools.insert(
        "probe".into(),
        ToolConfig::from_partial(config, vec![]).unwrap(),
    );
    let upstream = Client::default();
    let definitions = tool_definitions(cfg.conversation.tools.iter(), &upstream, None)
        .await
        .unwrap();
    let tools = definitions
        .into_iter()
        .map(|definition| ConfiguredTool {
            config: cfg.conversation.tools.get(&definition.name).unwrap(),
            definition,
            access: Ok(None),
            metadata:
                json!({"anthropic/maxResultSizeChars":500_000,"fixture/hint":{"opaque":true}})
                    .as_object()
                    .unwrap()
                    .clone(),
        })
        .collect();
    let (service, host) = Service::new(
        tools,
        upstream,
        builtins,
        root.path().to_owned(),
        InvocationContext {
            workspace_id: "workspace-1".into(),
            conversation_id: "conversation-1".into(),
        },
    )
    .unwrap();
    Fixture {
        endpoint: Endpoint::start(service).await.unwrap(),
        host,
        root,
    }
}

struct Ordinal(Arc<AtomicUsize>);
#[async_trait]
impl BuiltinTool for Ordinal {
    async fn execute(&self, _: &Value, _: &IndexMap<String, Value>) -> Outcome {
        format!("execution-{}", self.0.fetch_add(1, Ordering::SeqCst) + 1).into()
    }
}

async fn counting_fixture() -> (Fixture, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let fixture = fixture(
        json!({"source":"builtin", "summary":"Probe", "run":"ask", "result":"edit"}),
        BuiltinExecutors::new().register("probe", Ordinal(count.clone())),
    )
    .await;
    (fixture, count)
}

async fn next(host: &mut HostReceiver) -> HostRequest {
    timeout(Duration::from_secs(5), host.recv())
        .await
        .unwrap()
        .expect("Host channel closed")
}

#[tokio::test]
async fn external_discovery_preserves_host_metadata_without_executing() {
    let (mut fixture, count) = counting_fixture().await;
    let client = ExternalClient::connect(fixture.endpoint.url()).await;
    let result = client
        .request(1, "tools/list", json!({}))
        .await
        .reply(1)
        .await;
    assert_eq!(
        result,
        json!({"jsonrpc":"2.0","id":1,"result":{"tools":[{
            "name":"probe","description":"Probe","inputSchema":{"type":"object","properties":{},"required":[]},
            "_meta":{"anthropic/maxResultSizeChars":500_000,"fixture/hint":{"opaque":true}}
        }]}})
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(matches!(fixture.host.try_recv(), Err(TryRecvError::Empty)));
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test]
#[cfg(unix)]
#[expect(
    clippy::too_many_lines,
    reason = "Keep the inquiry, re-execution, and recording assertions in one linear scenario"
)]
async fn external_inquiry_reexecutes_with_host_answers_and_records_edited_output() {
    let mut fixture = fixture(json!({
        "source":"local", "run":"ask", "result":"edit", "options":{"marker":"configured"},
        "parameters":{"value":{"type":"string","required":true}},
        "command":{"program":"sh","shell":false,"args":["-c",
            "printf 'attempt\\n' >> attempts; if [ \"$1\" = null ]; then printf '%s' '{\"type\":\"needs_input\",\"question\":{\"id\":\"confirm\",\"text\":\"Continue?\",\"answer_type\":{\"type\":\"boolean\"}}}'; else printf '%s' \"$2\"; fi",
            "fixture", "{{tool.answers.confirm}}",
            "{{ {'value':tool.arguments.value,'answer':tool.answers.confirm,'action':context.action,'workspace':context.workspace_id,'conversation':context.conversation_id,'marker':tool.options.marker} | tojson }}"
        ]}
    }), BuiltinExecutors::new()).await;
    let attacker = fixture.root.path().join("attacker");
    fs::create_dir(&attacker).unwrap();
    let client = ExternalClient::connect(fixture.endpoint.url()).await;
    let meta = json!({
        "claudecode/toolUseId":"external-1",
        "computer.jp/context":{"root":attacker,"workspace_id":"forged","conversation_id":"forged","action":"format_arguments"},
        "computer.jp/tool":{"answers":{"confirm":true},"options":{"marker":"forged"}},
        "anthropic/maxResultSizeChars":0
    });
    let response = client
        .request(
            11,
            "tools/call",
            json!({"name":"probe","arguments":{"value":"requested"},"_meta":meta}),
        )
        .await;
    let pending = next(&mut fixture.host).await;
    let id = pending.call.id;
    assert_eq!(Value::Object(pending.call.request.correlation), meta);
    let Interaction::Prepare {
        arguments, reply, ..
    } = pending.interaction
    else {
        panic!("expected preparation")
    };
    assert_eq!(
        arguments,
        json!({"value":"requested"}).as_object().unwrap().clone()
    );
    assert!(!fixture.root.path().join("attempts").exists());
    reply
        .send(Ok(Admission::Run {
            arguments: json!({"value":"edited"}).as_object().unwrap().clone(),
        }))
        .unwrap();
    let pending = next(&mut fixture.host).await;
    assert_eq!(pending.call.id, id);
    let Interaction::Release { reply, .. } = pending.interaction else {
        panic!("expected release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let pending = next(&mut fixture.host).await;
    assert_eq!(pending.call.id, id);
    let Interaction::Input {
        request,
        answers,
        reply,
        ..
    } = pending.interaction
    else {
        panic!("caller metadata must not answer the inquiry")
    };
    assert_eq!(request.id.as_str(), "confirm");
    assert_eq!(
        request.schema,
        json!({"type":"boolean"}).as_object().unwrap().clone()
    );
    assert!(answers.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("attempts")).unwrap(),
        "attempt\n"
    );
    reply.send(Ok(InputAnswer::Answer(json!(false)))).unwrap();
    let pending = next(&mut fixture.host).await;
    assert_eq!(pending.call.id, id);
    let Interaction::Review { result, reply, .. } = pending.interaction else {
        panic!("expected review")
    };
    assert!(!result.is_error());
    let raw = result.to_text();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap(),
        json!({"value":"edited","answer":false,"action":"run","workspace":"workspace-1","conversation":"conversation-1","marker":"configured"})
    );
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("attempts")).unwrap(),
        "attempt\nattempt\n"
    );
    assert!(!attacker.join("attempts").exists());
    reply.send(Ok(ToolResult::text("approved output"))).unwrap();
    let pending = next(&mut fixture.host).await;
    assert_eq!(pending.call.id, id);
    assert_eq!(
        pending.call.request.arguments,
        json!({"value":"requested"}).as_object().unwrap().clone()
    );
    let Interaction::Record {
        arguments,
        raw_result,
        result,
        reply,
    } = pending.interaction
    else {
        panic!("expected record barrier")
    };
    assert_eq!(
        arguments,
        json!({"value":"edited"}).as_object().unwrap().clone()
    );
    assert_eq!(raw_result, Some(ToolResult::text(raw)));
    assert_eq!(result, ToolResult::text("approved output"));
    let mut returned = tokio::spawn(response.reply(11));
    assert!(
        timeout(Duration::from_millis(40), &mut returned)
            .await
            .is_err()
    );
    assert!(!reply.is_closed());
    fs::write(fixture.root.path().join("record.json"), serde_json::to_vec(&json!({"requested":pending.call.request.arguments,"executed":arguments,"result":result.to_text()})).unwrap()).unwrap();
    reply.send(Ok(())).unwrap();
    assert_eq!(
        returned.await.unwrap(),
        json!({"jsonrpc":"2.0","id":11,"result":{"content":[{"type":"text","text":"approved output"}],"isError":false}})
    );
    let stored: Value =
        serde_json::from_slice(&fs::read(fixture.root.path().join("record.json")).unwrap())
            .unwrap();
    assert_eq!(
        stored,
        json!({"requested":{"value":"requested"},"executed":{"value":"edited"},"result":"approved output"})
    );
    let listing = client
        .request(12, "tools/list", json!({}))
        .await
        .reply(12)
        .await;
    assert_eq!(
        listing["result"]["tools"][0]["_meta"],
        json!({"anthropic/maxResultSizeChars":500_000,"fixture/hint":{"opaque":true}})
    );
    client.close().await;
    fixture.shutdown().await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "The interleaved calls and their replies are asserted in protocol order"
)]
async fn host_and_external_client_share_handlers_without_sharing_call_identity() {
    let (mut fixture, count) = counting_fixture().await;
    let host_client = fixture.endpoint.connect().await.unwrap();
    let external = ExternalClient::connect(fixture.endpoint.url()).await;
    let mut params = CallToolRequestParams::new("probe");
    params.arguments = Some(Map::new());
    params.meta = Some(Meta(
        json!({"fixture/call":"host"}).as_object().unwrap().clone(),
    ));
    let peer = host_client.peer().clone();
    let host_result = tokio::spawn(async move { peer.call_tool(params).await.unwrap() });
    let first = next(&mut fixture.host).await;
    let first_id = first.call.id;
    assert_eq!(first.call.request.correlation["fixture/call"], "host");
    let Interaction::Prepare {
        reply: first_reply, ..
    } = first.interaction
    else {
        panic!("expected first preparation")
    };
    let external_response = external
        .request(
            17,
            "tools/call",
            json!({"name":"probe","arguments":{},"_meta":{"fixture/call":"external"}}),
        )
        .await;
    let second = next(&mut fixture.host).await;
    let second_id = second.call.id;
    assert_ne!(first_id, second_id);
    assert_eq!(
        second.call.request.correlation,
        json!({"fixture/call":"external"})
            .as_object()
            .unwrap()
            .clone()
    );
    assert_eq!(first.call.request.arguments, second.call.request.arguments);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let Interaction::Prepare {
        arguments, reply, ..
    } = second.interaction
    else {
        panic!("expected second preparation while first waits")
    };
    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let release = next(&mut fixture.host).await;
    assert_eq!(release.call.id, second_id);
    let Interaction::Release { reply, .. } = release.interaction else {
        panic!("expected second release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let review = next(&mut fixture.host).await;
    assert_eq!(review.call.id, second_id);
    let Interaction::Review { result, reply, .. } = review.interaction else {
        panic!("expected second review")
    };
    assert_eq!(result, ToolResult::text("execution-1"));
    reply.send(Ok(ToolResult::text("external result"))).unwrap();
    let record = next(&mut fixture.host).await;
    assert_eq!(record.call.id, second_id);
    let Interaction::Record { reply, .. } = record.interaction else {
        panic!("expected second record")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(
        external_response.reply(17).await,
        json!({"jsonrpc":"2.0","id":17,"result":{"content":[{"type":"text","text":"external result"}],"isError":false}})
    );
    assert!(!host_result.is_finished());
    assert!(!first_reply.is_closed());
    first_reply
        .send(Ok(Admission::Run {
            arguments: Map::new(),
        }))
        .unwrap();
    let release = next(&mut fixture.host).await;
    assert_eq!(release.call.id, first_id);
    let Interaction::Release { reply, .. } = release.interaction else {
        panic!("expected first release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let review = next(&mut fixture.host).await;
    assert_eq!(review.call.id, first_id);
    let Interaction::Review { result, reply, .. } = review.interaction else {
        panic!("expected first review")
    };
    assert_eq!(result, ToolResult::text("execution-2"));
    reply.send(Ok(ToolResult::text("host result"))).unwrap();
    let record = next(&mut fixture.host).await;
    assert_eq!(record.call.id, first_id);
    let Interaction::Record { reply, .. } = record.interaction else {
        panic!("expected first record")
    };
    reply.send(Ok(())).unwrap();
    assert_eq!(host_result.await.unwrap().content, vec![Content::text(
        "host result"
    )]);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    external.close().await;
    host_client.cancel().await.unwrap();
    fixture.shutdown().await;
}

#[tokio::test]
async fn disconnected_response_resumes_without_reexecuting_tool() {
    let (mut fixture, count) = counting_fixture().await;
    let external = ExternalClient::connect(fixture.endpoint.url()).await;
    let mut response = external
        .request(21, "tools/call", json!({"name":"probe","arguments":{}}))
        .await;
    let (event_id, data) = response.frame().await;
    assert_eq!(data, None);
    let event_id = event_id.expect("request stream must provide a resumption cursor");
    let prepared = next(&mut fixture.host).await;
    let id = prepared.call.id;
    let Interaction::Prepare {
        arguments, reply, ..
    } = prepared.interaction
    else {
        panic!("expected preparation")
    };
    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release { reply, .. } = next(&mut fixture.host).await.interaction else {
        panic!("expected release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let Interaction::Review { result, reply, .. } = next(&mut fixture.host).await.interaction
    else {
        panic!("expected review")
    };
    assert_eq!(result, ToolResult::text("execution-1"));
    reply.send(Ok(ToolResult::text("recorded result"))).unwrap();
    let record = next(&mut fixture.host).await;
    assert_eq!(record.call.id, id);
    let Interaction::Record { reply, .. } = record.interaction else {
        panic!("expected recording")
    };
    drop(response);
    let resumed = external.resume(&event_id).await;
    assert!(
        !reply.is_closed(),
        "HTTP disconnection must not cancel the invocation"
    );
    reply.send(Ok(())).unwrap();
    assert_eq!(
        resumed.reply(21).await,
        json!({"jsonrpc":"2.0","id":21,"result":{"content":[{"type":"text","text":"recorded result"}],"isError":false}})
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(matches!(fixture.host.try_recv(), Err(TryRecvError::Empty)));
    external.close().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn failed_recording_returns_error_instead_of_the_tool_result() {
    let (mut fixture, count) = counting_fixture().await;
    let external = ExternalClient::connect(fixture.endpoint.url()).await;
    let response = external
        .request(23, "tools/call", json!({"name":"probe","arguments":{}}))
        .await;
    let Interaction::Prepare {
        arguments, reply, ..
    } = next(&mut fixture.host).await.interaction
    else {
        panic!("expected preparation")
    };
    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let Interaction::Release { reply, .. } = next(&mut fixture.host).await.interaction else {
        panic!("expected release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
    let Interaction::Review { reply, .. } = next(&mut fixture.host).await.interaction else {
        panic!("expected review")
    };
    reply.send(Ok(ToolResult::text("approved result"))).unwrap();
    let Interaction::Record { reply, .. } = next(&mut fixture.host).await.interaction else {
        panic!("expected recording")
    };
    reply
        .send(Err(HostError::Recording(Arc::new(io::Error::other(
            "disk full",
        )))))
        .unwrap();
    assert_eq!(
        response.reply(23).await,
        json!({"jsonrpc":"2.0","id":23,"error":{"code":-32603,"message":"MCP Host operation failed: disk full"}})
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    external.close().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn host_loss_closes_an_outstanding_approval_without_execution() {
    let (mut fixture, count) = counting_fixture().await;
    let external = ExternalClient::connect(fixture.endpoint.url()).await;
    let response = external
        .request(25, "tools/call", json!({"name":"probe","arguments":{}}))
        .await;
    let Interaction::Prepare { mut reply, .. } = next(&mut fixture.host).await.interaction else {
        panic!("expected preparation")
    };
    drop(fixture.host);
    timeout(Duration::from_secs(5), reply.closed())
        .await
        .unwrap();
    assert!(
        reply
            .send(Ok(Admission::Run {
                arguments: Map::new()
            }))
            .is_err()
    );
    assert_eq!(
        response.reply(25).await,
        json!({"jsonrpc":"2.0","id":25,"error":{"code":-32603,"message":"MCP Host disconnected before completing the interaction"}})
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    external.close().await;
    fixture.endpoint.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_arguments_are_rejected_before_approval() {
    let count = Arc::new(AtomicUsize::new(0));
    let mut fixture = fixture(json!({"source":"builtin", "run":"ask", "parameters":{"value":{"type":"integer","required":true}}}), BuiltinExecutors::new().register("probe", Ordinal(count.clone()))).await;
    let external = ExternalClient::connect(fixture.endpoint.url()).await;
    let response = external
        .request(
            27,
            "tools/call",
            json!({"name":"probe","arguments":{"value":"not an integer"}}),
        )
        .await
        .reply(27)
        .await;
    assert_eq!(
        response,
        json!({"jsonrpc":"2.0","id":27,"error":{"code":-32602,"message":"Invalid tool argument at `value`"}})
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(matches!(fixture.host.try_recv(), Err(TryRecvError::Empty)));
    external.close().await;
    fixture.shutdown().await;
}

struct Inquiring(Arc<AtomicUsize>);
#[async_trait]
impl BuiltinTool for Inquiring {
    async fn execute(&self, _: &Value, answers: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        if answers.get("confirm") == Some(&json!(true)) {
            return "answered".into();
        }
        Question::boolean("confirm", "Continue?").unwrap().into()
    }
}

async fn release(host: &mut HostReceiver) {
    let request = next(host).await;
    let id = request.call.id;
    let Interaction::Prepare {
        arguments, reply, ..
    } = request.interaction
    else {
        panic!("expected preparation")
    };
    reply.send(Ok(Admission::Run { arguments })).unwrap();
    let request = next(host).await;
    assert_eq!(request.call.id, id);
    let Interaction::Release { reply, .. } = request.interaction else {
        panic!("expected release")
    };
    reply.send(Ok(ReleaseDecision::Execute)).unwrap();
}

#[tokio::test]
async fn cancellation_is_scoped_to_the_requesting_client_session() {
    let count = Arc::new(AtomicUsize::new(0));
    let mut fixture = fixture(
        json!({"source":"builtin", "run":"ask", "result":"unattended"}),
        BuiltinExecutors::new().register("probe", Inquiring(count.clone())),
    )
    .await;
    let first = ExternalClient::connect(fixture.endpoint.url()).await;
    let second = ExternalClient::connect(fixture.endpoint.url()).await;
    assert_ne!(first.session, second.session);
    let first_response = first
        .request(31, "tools/call", json!({"name":"probe","arguments":{}}))
        .await;
    release(&mut fixture.host).await;
    let first_input = next(&mut fixture.host).await;
    let Interaction::Input {
        reply: mut first_reply,
        ..
    } = first_input.interaction
    else {
        panic!("expected first input")
    };
    let second_response = second
        .request(31, "tools/call", json!({"name":"probe","arguments":{}}))
        .await;
    release(&mut fixture.host).await;
    let second_input = next(&mut fixture.host).await;
    assert_ne!(first_input.call.id, second_input.call.id);
    let Interaction::Input {
        reply: second_reply,
        ..
    } = second_input.interaction
    else {
        panic!("expected second input")
    };
    assert_eq!(count.load(Ordering::SeqCst), 2);
    first
        .notify(
            "notifications/cancelled",
            json!({"requestId":31,"reason":"fixture cancellation"}),
        )
        .await;
    timeout(Duration::from_secs(5), first_reply.closed())
        .await
        .unwrap();
    assert!(
        first_reply
            .send(Ok(InputAnswer::Answer(json!(true))))
            .is_err()
    );
    assert!(!second_reply.is_closed());
    second_reply
        .send(Ok(InputAnswer::Answer(json!(true))))
        .unwrap();
    let record = next(&mut fixture.host).await;
    assert_eq!(record.call.id, second_input.call.id);
    let Interaction::Record { result, reply, .. } = record.interaction else {
        panic!("expected only the second result")
    };
    assert_eq!(result, ToolResult::text("answered"));
    reply.send(Ok(())).unwrap();
    assert_eq!(
        second_response.reply(31).await,
        json!({"jsonrpc":"2.0","id":31,"result":{"content":[{"type":"text","text":"answered"}],"isError":false}})
    );
    assert_eq!(count.load(Ordering::SeqCst), 3);
    drop(first_response);
    first.close().await;
    second.close().await;
    fixture.shutdown().await;
}

struct LargeResult(String);
#[async_trait]
impl BuiltinTool for LargeResult {
    async fn execute(&self, _: &Value, _: &IndexMap<String, Value>) -> Outcome {
        self.0.clone().into()
    }
}

#[tokio::test]
async fn large_result_reaches_external_client_byte_for_byte() {
    // A repetitive fixed payload avoids a large checked-in fixture. Comparing
    // the entire value catches truncation, duplication, and newline changes.
    let payload = "line\n".repeat(48_000);
    assert_eq!(payload.len(), 240_000);
    let mut fixture = fixture(
        json!({"source":"builtin", "run":"ask", "result":"unattended"}),
        BuiltinExecutors::new().register("probe", LargeResult(payload.clone())),
    )
    .await;
    let external = ExternalClient::connect(fixture.endpoint.url()).await;
    let response = external
        .request(33, "tools/call", json!({"name":"probe","arguments":{}}))
        .await;
    release(&mut fixture.host).await;
    let Interaction::Record { result, reply, .. } = next(&mut fixture.host).await.interaction
    else {
        panic!("expected record")
    };
    assert_eq!(result, ToolResult::text(payload.clone()));
    reply.send(Ok(())).unwrap();
    let result = response.reply(33).await;
    assert_eq!(
        result,
        json!({"jsonrpc":"2.0","id":33,"result":{"content":[{"type":"text","text":payload}],"isError":false}})
    );
    external.close().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn external_denial_never_executes_the_tool() {
    let (mut fixture, count) = counting_fixture().await;
    let external = ExternalClient::connect(fixture.endpoint.url()).await;
    let response = external
        .request(35, "tools/call", json!({"name":"probe","arguments":{}}))
        .await;
    let Interaction::Prepare { reply, .. } = next(&mut fixture.host).await.interaction else {
        panic!("expected approval")
    };
    reply
        .send(Ok(Admission::Skip {
            reason: "denied by Host".into(),
        }))
        .unwrap();
    let Interaction::Record {
        raw_result, reply, ..
    } = next(&mut fixture.host).await.interaction
    else {
        panic!("expected recording without release")
    };
    assert_eq!(raw_result, None);
    reply.send(Ok(())).unwrap();
    assert_eq!(
        response.reply(35).await,
        json!({"jsonrpc":"2.0","id":35,"result":{"content":[{"type":"text","text":"denied by Host"}],"isError":false}})
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    external.close().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn edited_arguments_are_checked_before_execution_release() {
    let count = Arc::new(AtomicUsize::new(0));
    let mut fixture = fixture(json!({"source":"builtin", "run":"ask", "parameters":{"value":{"type":"integer","required":true}}}), BuiltinExecutors::new().register("probe", Ordinal(count.clone()))).await;
    let external = ExternalClient::connect(fixture.endpoint.url()).await;
    let response = external
        .request(
            37,
            "tools/call",
            json!({"name":"probe","arguments":{"value":1}}),
        )
        .await;
    let Interaction::Prepare { reply, .. } = next(&mut fixture.host).await.interaction else {
        panic!("expected valid initial arguments")
    };
    reply
        .send(Ok(Admission::Run {
            arguments: json!({"value":"invalid edit"}).as_object().unwrap().clone(),
        }))
        .unwrap();
    assert_eq!(
        response.reply(37).await,
        json!({"jsonrpc":"2.0","id":37,"error":{"code":-32602,"message":"Invalid tool argument at `value`"}})
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(matches!(fixture.host.try_recv(), Err(TryRecvError::Empty)));
    external.close().await;
    fixture.shutdown().await;
}
