//! Tests for how Claude Code handles an API connection drop mid-response.
//!
//! The test harness drives the ACP adapter with a handler that approves every
//! tool call, keeps the prompt request open, and records a summary of each SDK
//! message in the turn.
//! The recorded fixture (`tests/fixtures/acp/cut_off.jsonl`) shows that when a
//! response stream is interrupted after one tool call has completed, Claude
//! Code emits an assistant message with `error: "server_error"`, executes the
//! completed tool call, and sends a follow-up API request that re-issues the
//! interrupted tool call and finishes the turn.
//! Because Claude Code recovers on its own, JP holds `server_error` rather than
//! aborting the request on arrival;
//! [`a_turn_claude_code_continues_after_a_cut_completes`] replays the same
//! fixture through JP's protocol translation to verify that recovery.
//!
//! A second fixture (`cut_off_text.jsonl`) interrupts a response while text is
//! streaming and before any content block has completed.
//! Claude Code reports no error for that drop: it closes the text block, ends
//! the response, emits an `api_retry` system message, and re-issues the
//! request, which streams the text again from the beginning.
//! Because JP has already streamed and committed the first attempt's text, the
//! conversation ends up containing both the partial text and the retried
//! answer; [`a_response_claude_code_retries_keeps_the_text_of_both_attempts`]
//! replays the fixture to record that behavior.
//!
//! A third fixture (`cut_off_reasoned_text.jsonl`) interrupts a response's text
//! stream after its thinking block has completed.
//! Claude Code closes the text block, ends the response without an assistant
//! message for the interrupted text, and sends a synthetic user message asking
//! the model to resume, with no error and no `api_retry`.
//! The follow-up response writes the answer from the beginning, so JP again
//! commits the text of both attempts;
//! [`a_response_claude_code_resumes_keeps_the_text_of_both_attempts`] replays
//! the fixture to record that behavior.
//!
//! Committing the text of both attempts is tracked in T-1r592pw.
//!
//! When `RECORD=1` is set, the runtime connects to the Anthropic API through a
//! local forward proxy that drops all active `CONNECT` tunnels at the point
//! specified by [`Scenario::cut_at`] (simulating a connection drop such as
//! system sleep).
//! Without `RECORD`, the tests replay the fixtures without spawning a proxy,
//! tool server, or adapter process.
//! The fixtures also exercise `session/request_permission` and tool-call
//! session updates, which `live.jsonl` does not cover.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use camino::Utf8Path;
use datetime_literal::datetime;
use jp_config::{
    AppConfig,
    assistant::request::CachePolicy,
    model::parameters::{CustomReasoningConfig, ReasoningConfig, ReasoningEffort},
};
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ChatResponse, ConversationEvent},
    thread::ThreadBuilder,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Notify, broadcast, mpsc},
};

use super::{
    cassette::{self, Recorded},
    inspect, model_details, options,
    protocol::SdkNotification,
    recorded_tests::{context, only_connection, recording_login},
    rpc::{Handler, Inbound, RpcError},
    schema::{
        ClientCapabilities, InitializeRequest, NewSessionRequest, PermissionOptionKind,
        PromptRequest, ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SetSessionConfigOptionRequest, client_method, offered_models,
    },
    subscription_model,
    transcript::{PreparedRequest, tool_name},
    transport::{self, Foreground, NativeArtifact, Spawned, Transport},
};
use crate::{
    event::{Event, EventPart, FinishReason, ToolCallPart},
    event_builder::EventBuilder,
    query::{ChatQuery, QueryContext},
};

/// Tool whose call completes before the proxy drops the connection.
const LOOKUP: &str = "lookup";

/// Tool whose call is interrupted in the [`TOOL_CALL`] scenario.
const STORE: &str = "store";

/// Test scenario defining the prompt, model parameters, and point at which the
/// proxy drops the connection.
struct Scenario {
    fixture: &'static str,
    prompt: &'static str,
    reasoning: ReasoningConfig,
    /// Per-request output-token limit, sized to fit both the interrupted
    /// response and the retry (including reasoning tokens).
    max_tokens: u32,
    cut_at: CutAt,
}

/// Point in the response stream at which the proxy drops active tunnels.
#[derive(Clone, Copy)]
enum CutAt {
    /// When the [`STORE`] tool-call content block starts.
    StoreCall,

    /// On the [`TEXT_DELTAS_BEFORE_CUT`]th text delta after [`LOOKUP`] is
    /// dispatched.
    AnswerText,

    /// On the [`TEXT_DELTAS_BEFORE_CUT`]th text delta of a response that starts
    /// with a thinking block.
    TextAfterThinking,
}

/// Number of text deltas to let through before dropping a streaming text block.
const TEXT_DELTAS_BEFORE_CUT: usize = 5;

/// Two tool calls in a single response: a fast call (`lookup`) that completes
/// first, followed by a call (`store`) with a large argument payload so the
/// connection can be dropped while it streams.
const TOOL_CALL: Scenario = Scenario {
    fixture: "cut_off",
    prompt: "Make two tool calls in a single response, in this order. First call `lookup` with no \
             arguments. Then call `store`, with `text` set to the integers from 1 to 100 \
             separated by single spaces. Once both results arrive, reply with only the value that \
             `lookup` returned.",
    reasoning: ReasoningConfig::Off,
    max_tokens: 1024,
    cut_at: CutAt::StoreCall,
};

/// A tool call followed by an answer long enough for the connection to be
/// dropped while its text streams.
const TEXT: Scenario = Scenario {
    fixture: "cut_off_text",
    prompt: "Call `lookup` with no arguments. Once its result arrives, reply with only the \
             integers from 1 to 100 separated by single spaces.",
    reasoning: ReasoningConfig::Off,
    max_tokens: 1024,
    cut_at: CutAt::AnswerText,
};

/// A reasoned answer long enough for the connection to be dropped while its
/// text streams, after the response's thinking block has completed.
const REASONED_TEXT: Scenario = Scenario {
    fixture: "cut_off_reasoned_text",
    prompt: "Before you answer, think through which is larger: 17 * 23 or 19 * 21. Do not mention \
             the comparison in your answer. Then reply with only the integers from 1 to 100 \
             separated by single spaces.",
    reasoning: ReasoningConfig::Custom(CustomReasoningConfig {
        effort: ReasoningEffort::High,
        exclude: false,
    }),
    max_tokens: 2048,
    cut_at: CutAt::TextAfterThinking,
};

/// Expected text response for the [`TEXT`] and [`REASONED_TEXT`] scenarios.
const INTEGERS: &str = "1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 \
                        28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 \
                        52 53 54 55 56 57 58 59 60 61 62 63 64 65 66 67 68 69 70 71 72 73 74 75 \
                        76 77 78 79 80 81 82 83 84 85 86 87 88 89 90 91 92 93 94 95 96 97 98 99 \
                        100";

const CUT: &str = "cut every connection to the API";

/// Hosts bypassed by the proxy (`NO_PROXY`) so local MCP tool calls connect
/// directly.
const LOOPBACK: &str = "127.0.0.1,localhost";

type Outline = Arc<Mutex<Vec<Value>>>;

fn query(scenario: &Scenario) -> ChatQuery {
    let mut config = AppConfig::new_test();
    config.assistant.model.id = "anthropic/claude-opus-5".parse().unwrap();
    config.assistant.model.parameters.max_tokens = Some(scenario.max_tokens);
    config.assistant.model.parameters.reasoning = Some(scenario.reasoning);
    config.assistant.request.cache = CachePolicy::Off;
    let timestamp = datetime!(2026-11-24 12:00:00 Z);
    let mut events = ConversationStream::new(config.into()).with_created_at(timestamp);
    events.extend([ConversationEvent::new(
        ChatRequest::from(scenario.prompt),
        timestamp,
    )]);
    ThreadBuilder::new()
        .with_events(events)
        .build()
        .unwrap()
        .into()
}

/// Read the request line and body of an HTTP/1.1 request, or return `None` on
/// EOF.
async fn read_request(stream: &mut BufReader<TcpStream>) -> Option<(String, Vec<u8>)> {
    let mut first = String::new();
    let mut body_size = 0_usize;
    loop {
        let mut line = String::new();
        if stream.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if first.is_empty() {
            first = line.to_owned();
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            body_size = value.trim().parse().ok()?;
        }
    }

    let mut body = vec![0; body_size];
    stream.read_exact(&mut body).await.ok()?;
    Some((first, body))
}

/// HTTP `CONNECT` forward proxy that can drop all active tunnels on demand.
///
/// Tunnels opened after a `cut` signal are unaffected, allowing subsequent
/// retry requests from the runtime to reach the API.
struct Proxy {
    url: String,
    cut: broadcast::Sender<()>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Proxy {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (cut, _) = broadcast::channel(1);
        let log = Arc::<Mutex<Vec<String>>>::default();

        tokio::spawn({
            let cut = cut.clone();
            let log = log.clone();
            async move {
                while let Ok((client, _)) = listener.accept().await {
                    tokio::spawn(tunnel(client, cut.subscribe(), log.clone()));
                }
            }
        });

        Self { url, cut, log }
    }

    fn tunnels(&self) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.ends_with(": tunnelled"))
            .count()
    }
}

/// Relay one `CONNECT` tunnel until either peer closes the connection or `cut`
/// fires.
async fn tunnel(client: TcpStream, mut cut: broadcast::Receiver<()>, log: Arc<Mutex<Vec<String>>>) {
    let note = |line: String| log.lock().unwrap().push(line);
    let mut client = BufReader::new(client);
    let Some((request, _)) = read_request(&mut client).await else {
        return;
    };
    // `CONNECT api.anthropic.com:443 HTTP/1.1`
    let mut words = request.split_whitespace();
    let (Some("CONNECT"), Some(target)) = (words.next(), words.next()) else {
        note(format!("{request}: refused, not a CONNECT"));
        drop(
            client
                .write_all(b"HTTP/1.1 405 Method Not Allowed\r\ncontent-length: 0\r\n\r\n")
                .await,
        );
        return;
    };
    let mut upstream = match TcpStream::connect(target).await {
        Ok(upstream) => upstream,
        Err(error) => {
            note(format!("{request}: unreachable, {error}"));
            drop(
                client
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n")
                    .await,
            );
            return;
        }
    };
    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    note(format!("{request}: tunnelled"));

    // Returning drops both sockets, severing the connection.
    tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
        _ = cut.recv() => {}
    }
}

/// Start a stub MCP Streamable HTTP server for [`LOOKUP`] and [`STORE`] and
/// return its endpoint URL.
async fn tool_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());

    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            tokio::spawn(serve_tools(client));
        }
    });

    url
}

async fn serve_tools(client: TcpStream) {
    let mut client = BufReader::new(client);
    while let Some((_, body)) = read_request(&mut client).await {
        let response = match serde_json::from_slice::<Value>(&body) {
            Ok(message) => match answer(&message) {
                Some(answer) => http("200 OK", &answer.to_string()),
                // JSON-RPC notifications take an empty 202 response.
                None => http("202 Accepted", ""),
            },
            // Reject non-JSON requests (such as GET requests for a server-sent
            // event stream).
            Err(_) => http("405 Method Not Allowed", ""),
        };
        if client.write_all(response.as_bytes()).await.is_err() {
            return;
        }
    }
}

fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// Build a JSON-RPC response for an MCP request, or `None` for a notification.
fn answer(message: &Value) -> Option<Value> {
    let id = message.get("id")?;
    let result = match message["method"].as_str()? {
        "initialize" => json!({
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "jp", "version": "0.0.0"},
        }),
        "tools/list" => json!({"tools": [
            {
                "name": LOOKUP,
                "description": "Look up the active invoice ID.",
                "inputSchema": {"type": "object", "properties": {}},
            },
            {
                "name": STORE,
                "description": "Store a text.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                },
            },
        ]}),
        "tools/call" => {
            let text = if message["params"]["name"] == LOOKUP {
                "INV-1042"
            } else {
                "Stored."
            };
            json!({"content": [{"type": "text", "text": text}], "isError": false})
        }
        // Empty response for `ping` and any other method.
        _ => json!({}),
    };

    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

struct Probe {
    transport: Box<dyn Transport>,
    environment: BTreeMap<String, String>,
    tools: String,
    cut: broadcast::Sender<()>,
    cut_at: CutAt,
    /// Idle duration required after the prompt finishes before ending capture.
    quiet: Duration,
    /// Maximum duration to wait for a turn result.
    limit: Duration,
}

/// Configure a live probe that spawns the installed adapter behind `proxy` with
/// a local stub tool server, recording into the fixture of `scenario`.
async fn live(
    prepared: &PreparedRequest,
    proxy: &Proxy,
    login: Option<&Utf8Path>,
    scenario: &Scenario,
) -> Probe {
    let mut launch = transport::launch(prepared, &context(), CachePolicy::Off, login)
        .expect("a prepared launch");

    // Set both uppercase and lowercase proxy variables on the adapter process
    // and in the session options, since Claude Code checks the lowercase form
    // first.
    for (name, value) in [
        ("HTTPS_PROXY", proxy.url.as_str()),
        ("https_proxy", proxy.url.as_str()),
        ("NO_PROXY", LOOPBACK),
        ("no_proxy", LOOPBACK),
    ] {
        launch.command.env(name, value);
        launch.environment.insert(name.to_owned(), value.to_owned());
    }

    Probe {
        transport: Box::new(Spawned {
            command: launch.command,
            tap: cassette::tap(scenario.fixture),
        }),
        environment: launch.environment,
        tools: tool_server().await,
        cut: proxy.cut.clone(),
        cut_at: scenario.cut_at,
        // Wait long enough to catch any follow-up request started after a
        // result message.
        quiet: Duration::from_secs(20),
        limit: Duration::from_mins(5),
    }
}

/// Configure a probe that replays the fixture of `scenario` without network or
/// process I/O.
fn replayed(prepared: &PreparedRequest, scenario: &Scenario) -> Probe {
    Probe {
        transport: Box::new(Recorded(only_connection(scenario.fixture))),
        environment: options::environment(prepared, CachePolicy::Off),
        tools: "http://127.0.0.1:1/mcp".to_owned(),
        cut: broadcast::channel(1).0,
        cut_at: scenario.cut_at,
        quiet: Duration::from_millis(250),
        limit: Duration::from_secs(5),
    }
}

fn excerpt(text: &str) -> String {
    text.chars().take(120).collect()
}

/// Summarize a message's content blocks for snapshot comparison.
fn blocks(content: &Value) -> Value {
    // A user message may hold a bare string.
    if let Some(text) = content.as_str() {
        return json!([{"text": excerpt(text)}]);
    }

    content
        .as_array()
        .into_iter()
        .flatten()
        .map(|block| match block["type"].as_str() {
            Some("text") => json!({"text": excerpt(block["text"].as_str().unwrap_or_default())}),
            Some("tool_use") => json!({"tool_use": block["name"]}),
            Some("tool_result") => json!({"tool_result": excerpt(&block["content"].to_string())}),
            _ => block["type"].clone(),
        })
        .collect()
}

/// Build a snapshot summary entry for an SDK message, or `None` for content
/// delta events.
///
/// Omits unstable fields that vary across recordings (IDs, timestamps, token
/// usage, streaming deltas, and `thinking_tokens` progress messages) as well as
/// null fields.
fn entry(message: &Value) -> Option<Value> {
    let mut entry = match message["type"].as_str()? {
        "stream_event" => stream_entry(&message["event"])?,
        "assistant" => json!({
            "assistant": blocks(&message["message"]["content"]),
            "error": message["error"],
            "aborted": message["aborted"],
        }),
        "user" => json!({
            "user": blocks(&message["message"]["content"]),
            "synthetic": message["isSynthetic"],
        }),
        "result" => json!({
            "result": message["subtype"],
            "is_error": message["is_error"],
            "stop_reason": message["stop_reason"],
            "terminal_reason": message["terminal_reason"],
            "api_error_status": message["api_error_status"],
            "errors": message["errors"],
        }),
        "system" if message["subtype"] == "thinking_tokens" => return None,
        "system" => json!({
            "system": message["subtype"],
            "tools": message["tools"],
            "attempt": message["attempt"],
            "error": message["error"],
        }),
        other => json!({"other": other}),
    };

    if let Some(fields) = entry.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
    Some(entry)
}

fn stream_entry(event: &Value) -> Option<Value> {
    Some(match event["type"].as_str()? {
        "message_start" | "message_stop" | "content_block_stop" => {
            json!({"stream": event["type"]})
        }
        "content_block_start" => json!({
            "stream": "content_block_start",
            "block": event["content_block"]["type"],
            "tool": event["content_block"]["name"],
        }),
        "message_delta" => json!({
            "stream": "message_delta",
            "stop_reason": event["delta"]["stop_reason"],
        }),
        _ => return None,
    })
}

fn opens_store_call(message: &Value) -> bool {
    let event = &message["event"];
    message["type"] == "stream_event"
        && event["type"] == "content_block_start"
        && event["content_block"]["name"].as_str() == Some(tool_name(STORE).as_str())
}

fn is_text_delta(message: &Value) -> bool {
    let event = &message["event"];
    message["type"] == "stream_event"
        && event["type"] == "content_block_delta"
        && event["delta"]["type"] == "text_delta"
}

fn starts_response(message: &Value) -> bool {
    message["type"] == "stream_event" && message["event"]["type"] == "message_start"
}

fn opens_thinking_block(message: &Value) -> bool {
    let event = &message["event"];
    message["type"] == "stream_event"
        && event["type"] == "content_block_start"
        && event["content_block"]["type"] == "thinking"
}

/// Tracks stream state to trigger the proxy connection drop at `cut_at` once
/// per turn.
struct Trigger {
    cut_at: CutAt,
    lookup_dispatched: bool,
    thinking_seen: bool,
    text_deltas: usize,
    fired: bool,
}

impl Trigger {
    fn new(cut_at: CutAt) -> Self {
        Self {
            cut_at,
            lookup_dispatched: false,
            thinking_seen: false,
            text_deltas: 0,
            fired: false,
        }
    }

    fn dispatched(&mut self, tool: Option<&Value>) {
        self.lookup_dispatched |= tool == Some(&json!(tool_name(LOOKUP)));
    }

    fn fires_on(&mut self, message: &Value) -> bool {
        if self.fired {
            return false;
        }

        self.fired = match self.cut_at {
            CutAt::StoreCall => opens_store_call(message),
            CutAt::AnswerText => {
                if self.lookup_dispatched && is_text_delta(message) {
                    self.text_deltas += 1;
                }
                self.text_deltas == TEXT_DELTAS_BEFORE_CUT
            }
            CutAt::TextAfterThinking => {
                if starts_response(message) {
                    self.thinking_seen = false;
                    self.text_deltas = 0;
                }
                self.thinking_seen |= opens_thinking_block(message);
                if self.thinking_seen && is_text_delta(message) {
                    self.text_deltas += 1;
                }
                self.text_deltas == TEXT_DELTAS_BEFORE_CUT
            }
        };
        self.fired
    }
}

/// Build a handler that records SDK messages into `outline`, approves every
/// tool permission request, and triggers `cut` at `cut_at`.
///
/// Unlike `transport::inbound`, this handler acknowledges error SDK messages
/// without aborting so the session stays open to observe Claude Code's
/// recovery.
fn observer(
    outline: Outline,
    activity: Arc<Notify>,
    finished: Arc<Notify>,
    cut: broadcast::Sender<()>,
    cut_at: CutAt,
) -> Handler {
    let trigger = Arc::new(Mutex::new(Trigger::new(cut_at)));

    Box::new(move |inbound| {
        let outline = outline.clone();
        let activity = activity.clone();
        let finished = finished.clone();
        let cut = cut.clone();
        let trigger = trigger.clone();

        Box::pin(async move {
            let (Inbound::Notification { method, params } | Inbound::Request { method, params }) =
                inbound;
            activity.notify_one();

            if method == SdkNotification::METHOD {
                let message = &params["message"];
                let mut entries = outline.lock().unwrap();
                entries.extend(entry(message));
                if trigger.lock().unwrap().fires_on(message) {
                    // Record the cut marker in both live and replay runs so the
                    // resulting outline matches the snapshot.
                    drop(cut.send(()));
                    entries.push(json!({"probe": CUT}));
                }
                drop(entries);

                if message["type"] == "result" {
                    finished.notify_one();
                }
                return Ok(Value::Null);
            }

            if method == client_method::SESSION_REQUEST_PERMISSION {
                let request: RequestPermissionRequest =
                    serde_json::from_value(params).map_err(RpcError::into_internal_error)?;
                let tool = request
                    .tool_call
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("claudeCode"))
                    .and_then(|claude| claude.get("toolName"))
                    .cloned();
                trigger.lock().unwrap().dispatched(tool.as_ref());
                outline.lock().unwrap().push(json!({"permission": tool}));

                let option = request
                    .options
                    .into_iter()
                    .find(|option| option.kind == PermissionOptionKind::AllowOnce)
                    .ok_or_else(|| {
                        RpcError::into_internal_error("no one-call permission was offered")
                    })?;
                return serde_json::to_value(RequestPermissionResponse {
                    outcome: RequestPermissionOutcome::Selected {
                        option_id: option.option_id,
                    },
                })
                .map_err(RpcError::into_internal_error);
            }

            Ok(Value::Null)
        })
    })
}

/// Wait until the adapter has sent nothing for `quiet`.
async fn silence(activity: &Notify, quiet: Duration) {
    while tokio::time::timeout(quiet, activity.notified())
        .await
        .is_ok()
    {}
}

/// Send the prepared prompt in a new session and return the recorded message
/// outline.
async fn observe(probe: Probe, prepared: PreparedRequest) -> Vec<Value> {
    let outline: Outline = Arc::default();
    let activity = Arc::new(Notify::new());
    let finished = Arc::new(Notify::new());
    let handler = observer(
        outline.clone(),
        activity.clone(),
        finished.clone(),
        probe.cut,
        probe.cut_at,
    );

    let options = options::metadata(&prepared, &probe.environment).unwrap();
    let PreparedRequest { prompt, model, .. } = prepared;
    let root = context().root;
    let (tools, quiet, limit) = (probe.tools, probe.quiet, probe.limit);

    let foreground: Foreground = {
        let outline = outline.clone();
        Box::new(move |peer| {
            Box::pin(async move {
                peer.request(InitializeRequest {
                    protocol_version: ProtocolVersion::V1,
                    client_capabilities: ClientCapabilities::default(),
                })
                .await?;

                let request: NewSessionRequest = serde_json::from_value(json!({
                    "cwd": root,
                    "mcpServers": [{"type": "http", "name": "jp", "url": tools, "headers": []}],
                    "_meta": options,
                }))
                .map_err(RpcError::into_internal_error)?;
                let session = peer.request(request).await?;

                let model = subscription_model(&model, &offered_models(&session.config_options));
                for (setting, value) in [("model", model.as_str()), ("mode", "default")] {
                    let request: SetSessionConfigOptionRequest = serde_json::from_value(json!({
                        "sessionId": session.session_id,
                        "configId": setting,
                        "value": value,
                    }))
                    .map_err(RpcError::into_internal_error)?;
                    peer.request(request).await?;
                }

                let request: PromptRequest = serde_json::from_value(json!({
                    "sessionId": session.session_id,
                    "prompt": [{"type": "text", "text": prompt}],
                }))
                .map_err(RpcError::into_internal_error)?;
                let answer = peer.request(request);
                tokio::pin!(answer);

                // Wait for `quiet` of inactivity before recording the final
                // probe event so any trailing notifications are captured first.
                let ending = tokio::select! {
                    answer = &mut answer => {
                        answer?;
                        silence(&activity, quiet).await;
                        "the adapter answered the prompt"
                    }
                    () = async {
                        finished.notified().await;
                        silence(&activity, quiet).await;
                    } => "a result arrived and the prompt stayed open",
                    () = tokio::time::sleep(limit) => "no result arrived",
                };
                outline.lock().unwrap().push(json!({"probe": ending}));

                Ok(())
            })
        })
    };

    tokio::time::timeout(limit * 2, probe.transport.connect(handler, foreground))
        .await
        .expect("the adapter never got as far as the prompt")
        .unwrap();

    outline.lock().unwrap().clone()
}

/// Return the outline of the turn `scenario` describes: recorded against the
/// installed runtime under `RECORD`, replayed from its fixture otherwise.
async fn outline(scenario: &Scenario) -> Vec<Value> {
    let model = model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query(scenario)).unwrap();

    if !cassette::recording() {
        return observe(replayed(&prepared, scenario), prepared).await;
    }

    let login = recording_login();
    inspect(login.as_deref())
        .await
        .expect("a qualified adapter and an active login");

    let proxy = Proxy::start().await;
    let probe = live(&prepared, &proxy, login.as_deref(), scenario).await;
    let turn = observe(probe, prepared).await;

    println!("{:#}", json!(turn));
    println!("{:#}", json!(*proxy.log.lock().unwrap()));
    assert!(
        proxy.tunnels() > 0,
        "Claude Code never reached the API through the proxy, so no connection was cut"
    );
    turn
}

fn position(turn: &[Value], found: impl Fn(&Value) -> bool) -> Option<usize> {
    turn.iter().position(found)
}

/// Replay `scenario`'s fixture through `transport::drive` and return the
/// resulting stream events.
async fn driven(scenario: &Scenario) -> Vec<Event> {
    let model = model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query(scenario)).unwrap();
    let environment = options::environment(&prepared, CachePolicy::Off);
    let (sender, mut receiver) = mpsc::channel(64);
    let driver = transport::drive(
        prepared,
        QueryContext {
            root: context().root,
            mcp_endpoint: Some("http://127.0.0.1:1/mcp".parse().unwrap()),
            invocation: None,
        },
        vec![LOOKUP.to_owned(), STORE.to_owned()],
        environment,
        NativeArtifact::resume(None),
        Box::new(Recorded(only_connection(scenario.fixture))),
        sender,
    );
    let collect = async {
        let mut events = vec![];
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    };

    let (result, events) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(driver, collect)
    })
    .await
    .expect("the replay did not finish");

    result.unwrap();
    events.into_iter().collect::<Result<Vec<_>, _>>().unwrap()
}

/// Return the assistant message texts committed by `events` via
/// [`EventBuilder`], in commit order.
fn committed_messages(events: Vec<Event>) -> Vec<String> {
    let mut builder = EventBuilder::new();
    let mut committed = vec![];
    for event in events {
        match event {
            Event::Part {
                index,
                part,
                metadata,
            } => builder.handle_part(index, part, metadata),
            Event::Flush { index, metadata } => {
                committed.extend(builder.handle_flush(index, metadata).ok().flatten());
            }
            Event::Finished(_) => committed.extend(builder.drain()),
            _ => {}
        }
    }

    committed
        .into_iter()
        .filter_map(ConversationEvent::into_chat_response)
        .filter_map(|response| match response {
            ChatResponse::Message { message } => Some(message),
            _ => None,
        })
        .collect()
}

/// Verify that Claude Code recovers when the API connection drops mid-response
/// after a completed tool call.
///
/// In the snapshot, the cut is followed by a `message_stop` with no
/// `content_block_stop` for the `store` block, and the `assistant` entry with
/// `"error": "server_error"` is followed by the execution of `lookup`, a new
/// `message_start` request, the re-issued `store` tool call, and a final
/// `result` with `"subtype": "success"`.
///
/// To re-record against a live subscription:
///
/// ```sh
/// RECORD=1 cargo test -p jp_llm turn_after_a_cut_off_response -- --nocapture
/// ```
#[tokio::test]
async fn turn_after_a_cut_off_response() {
    let turn = outline(&TOOL_CALL).await;

    // Verify that `lookup` was dispatched before the connection drop and that
    // the drop produced an assistant `server_error` message.
    let lookup = position(&turn, |entry| {
        entry["permission"] == json!(tool_name(LOOKUP))
    });
    let cut = position(&turn, |entry| entry.get("probe") == Some(&json!(CUT)));
    let error = position(&turn, |entry| {
        entry.get("assistant").is_some() && entry.get("error").is_some()
    });
    assert!(
        matches!((lookup, cut, error), (Some(lookup), Some(cut), Some(error)) if lookup < cut && cut < error),
        "expected `lookup` to be dispatched, then the cut, then an assistant message carrying an \
         error; got them at {lookup:?}, {cut:?}, {error:?}: {turn:#?}"
    );

    insta::assert_json_snapshot!("turn_after_a_cut_off_response", turn);
}

/// Replay the [`TOOL_CALL`] fixture through `transport::drive` and verify that
/// JP ignores the intermediate `server_error`, dispatches [`LOOKUP`] and
/// [`STORE`] once each, and finishes the turn with [`FinishReason::Completed`].
#[tokio::test]
async fn a_turn_claude_code_continues_after_a_cut_completes() {
    // Under `RECORD`, the recording test is rewriting the fixture this replay
    // reads.
    if cassette::recording() {
        return;
    }

    let events = driven(&TOOL_CALL).await;

    let calls = events
        .iter()
        .filter_map(|event| match event {
            Event::Part {
                part: EventPart::ToolCall(ToolCallPart::Start { name, .. }),
                ..
            } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls, [LOOKUP, STORE]);
    assert_eq!(
        events.last(),
        Some(&Event::Finished(FinishReason::Completed))
    );
}

/// Verify that Claude Code retries automatically when the API connection drops
/// while text is streaming and before any content block has completed.
///
/// In the snapshot, the cut is followed by `content_block_stop` and
/// `message_stop` (with no `message_delta` between them), an `api_retry` system
/// message, and a new `message_start` that streams the text again from the
/// beginning without emitting an assistant error message.
///
/// To re-record against a live subscription:
///
/// ```sh
/// RECORD=1 cargo test -p jp_llm turn_after_a_cut_off_text_block -- --nocapture
/// ```
#[tokio::test]
async fn turn_after_a_cut_off_text_block() {
    let turn = outline(&TEXT).await;

    // Verify that a new response started after the connection drop.
    let cut = position(&turn, |entry| entry.get("probe") == Some(&json!(CUT)));
    let resumed = turn
        .iter()
        .rposition(|entry| entry["stream"] == "message_start");
    assert!(
        matches!((cut, resumed), (Some(cut), Some(resumed)) if cut < resumed),
        "expected the cut, then a response started after it; got them at {cut:?}, {resumed:?}: \
         {turn:#?}"
    );

    insta::assert_json_snapshot!("turn_after_a_cut_off_text_block", turn);
}

/// Replay the [`TEXT`] fixture through `transport::drive` and verify that JP
/// commits the partial text from the interrupted attempt followed by the full
/// text from Claude Code's retry (T-1r592pw).
#[tokio::test]
async fn a_response_claude_code_retries_keeps_the_text_of_both_attempts() {
    if cassette::recording() {
        return;
    }

    assert_keeps_the_text_of_both_attempts(driven(&TEXT).await);
}

/// Assert that `events` end in [`FinishReason::Completed`] and that the last
/// two committed messages are a non-empty prefix of [`INTEGERS`] followed by
/// the full [`INTEGERS`] string.
fn assert_keeps_the_text_of_both_attempts(events: Vec<Event>) {
    assert_eq!(
        events.last(),
        Some(&Event::Finished(FinishReason::Completed))
    );

    let messages = committed_messages(events);
    let [.., interrupted, answer] = messages.as_slice() else {
        panic!("expected the interrupted text and the answer: {messages:#?}");
    };
    assert_eq!(answer, INTEGERS);

    let interrupted = interrupted.trim_end();
    assert!(
        !interrupted.is_empty() && interrupted != INTEGERS && INTEGERS.starts_with(interrupted),
        "expected the start of the answer, cut short: {interrupted:?}"
    );
}

/// Verify that Claude Code sends a synthetic resume prompt when the API
/// connection drops while text is streaming after a completed thinking block.
///
/// In the snapshot, the cut is followed by `content_block_stop` and
/// `message_stop` (with no `message_delta` between them), then a synthetic
/// `user` message asking the model to resume.
/// No assistant message is emitted for the interrupted text, and the next
/// response reasons again and writes the text from the beginning.
///
/// To re-record against a live subscription:
///
/// ```sh
/// RECORD=1 cargo test -p jp_llm turn_after_a_cut_off_reasoned_text -- --nocapture
/// ```
#[tokio::test]
async fn turn_after_a_cut_off_reasoned_text() {
    let turn = outline(&REASONED_TEXT).await;

    let thinking = position(&turn, |entry| entry["block"] == "thinking");
    let cut = position(&turn, |entry| entry.get("probe") == Some(&json!(CUT)));
    let resume = position(&turn, |entry| entry["synthetic"] == true);
    assert!(
        matches!((thinking, cut, resume), (Some(thinking), Some(cut), Some(resume)) if thinking < cut && cut < resume),
        "expected a thinking block, then the cut, then a synthetic user message; got them at \
         {thinking:?}, {cut:?}, {resume:?}: {turn:#?}"
    );

    insta::assert_json_snapshot!("turn_after_a_cut_off_reasoned_text", turn);
}

/// Replay the [`REASONED_TEXT`] fixture through `transport::drive` and verify
/// that JP commits the partial text from the interrupted attempt followed by
/// the full text from the resumed response (T-1r592pw).
#[tokio::test]
async fn a_response_claude_code_resumes_keeps_the_text_of_both_attempts() {
    if cassette::recording() {
        return;
    }

    assert_keeps_the_text_of_both_attempts(driven(&REASONED_TEXT).await);
}
