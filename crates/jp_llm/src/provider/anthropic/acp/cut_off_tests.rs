//! What Claude Code does with a turn after it loses its connection to the API
//! partway through a response.
//!
//! The probe drives the adapter with a handler that refuses nothing, leaves the
//! prompt open, and outlines every SDK message the turn produces.
//! The recording shows Claude Code reporting the loss as an assistant message
//! carrying `error: "server_error"`, then running the call that completed
//! before the cut and asking the model again, which issues the interrupted call
//! anew and finishes the turn.
//! That is why JP holds the report rather than ending the request on it, and
//! [`a_turn_claude_code_continues_after_a_cut_completes`] replays the same
//! recording through JP's own translation to pin that.
//!
//! Under `RECORD=1` the runtime reaches the API through a proxy this file runs,
//! and the proxy drops its tunnels at a chosen point in the response.
//! Claude Code documents a connection broken by system sleep as one it counts
//! as dropped, so the cut stands in for closing the laptop lid.
//! The model is asked for two tool calls in one response and the cut lands
//! while the second is being written: the first is complete by then, which is
//! the case Claude Code reports to its caller instead of retrying out of sight.
//!
//! Without `RECORD` the probe replays `tests/fixtures/acp/cut_off.jsonl`
//! through the same handler, with no proxy, no tool server, and no runtime.
//! That recording also carries `session/request_permission` and the tool-call
//! session updates, which `live.jsonl` has none of.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use camino::Utf8Path;
use datetime_literal::datetime;
use jp_config::{AppConfig, assistant::request::CachePolicy, model::parameters::ReasoningConfig};
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ConversationEvent},
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
    query::{ChatQuery, QueryContext},
};

/// The recording this probe writes and replays.
const FIXTURE: &str = "cut_off";

/// The tool whose call completes before the cut.
const LOOKUP: &str = "lookup";

/// The tool whose call the cut interrupts.
const STORE: &str = "store";

/// Asks for two tool calls in one response: a short one that completes, then
/// one whose argument takes a few seconds to write, which is the window the cut
/// lands in.
const PROMPT: &str = "Make two tool calls in a single response, in this order. First call \
                      `lookup` with no arguments. Then call `store`, with `text` set to the \
                      integers from 1 to 100 separated by single spaces. Once both results \
                      arrive, reply with only the value that `lookup` returned.";

/// Hosts the proxy must not be used for.
///
/// The tool server listens on loopback, and the proxy only tunnels.
const LOOPBACK: &str = "127.0.0.1,localhost";

/// What the turn produced, in the order it arrived.
type Outline = Arc<Mutex<Vec<Value>>>;

fn query() -> ChatQuery {
    let mut config = AppConfig::new_test();
    config.assistant.model.id = "anthropic/claude-opus-5".parse().unwrap();
    // Room for the argument twice over, since the runtime asks for the
    // interrupted call again.
    config.assistant.model.parameters.max_tokens = Some(1024);
    config.assistant.model.parameters.reasoning = Some(ReasoningConfig::Off);
    config.assistant.request.cache = CachePolicy::Off;
    let timestamp = datetime!(2026-11-24 12:00:00 Z);
    let mut events = ConversationStream::new(config.into()).with_created_at(timestamp);
    events.extend([ConversationEvent::new(ChatRequest::from(PROMPT), timestamp)]);
    ThreadBuilder::new()
        .with_events(events)
        .build()
        .unwrap()
        .into()
}

/// Read one HTTP/1.1 request: its first line and its body.
///
/// Returns `None` when the client goes away before finishing one.
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

/// A forward proxy for `CONNECT` tunnels that can drop every tunnel it has
/// open.
///
/// A tunnel opened after a cut is left alone, so whatever the runtime does next
/// reaches the API.
struct Proxy {
    url: String,

    /// Drops the tunnels open at the moment it is sent on.
    cut: broadcast::Sender<()>,

    /// Every request the proxy received, with what came of it, which says
    /// whether the runtime used the proxy at all and what it reached through
    /// it.
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

    /// How many tunnels reached their target.
    fn tunnels(&self) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.ends_with(": tunnelled"))
            .count()
    }
}

/// Carry one `CONNECT` tunnel until either end closes it or `cut` fires.
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

    // Returning closes both sockets, which is the cut.
    tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
        _ = cut.recv() => {}
    }
}

/// Serve [`LOOKUP`] and [`STORE`] over MCP's Streamable HTTP transport, and
/// return the URL to reach them at.
///
/// A stand-in for JP's tool server, which lives above this crate.
/// It answers every request with a single JSON body and opens no stream of its
/// own, which is the least the transport allows.
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
                // A notification, which is acknowledged without a body.
                None => http("202 Accepted", ""),
            },
            // Only a POST carries a message. A GET asks for a stream this
            // server never has anything to send on.
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

/// Answer one MCP request, or return `None` for a message that expects no
/// answer.
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
        // `ping`, the one other request a client sends a server that offers
        // nothing but tools.
        _ => json!({}),
    };

    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// One way of reaching an adapter, and how patiently to watch it.
struct Probe {
    transport: Box<dyn Transport>,

    /// The environment the session options hand the runtime.
    environment: BTreeMap<String, String>,

    /// Where the session's one MCP server listens.
    tools: String,

    /// Drops the runtime's connections to the API.
    cut: broadcast::Sender<()>,

    /// How long the adapter has to stay silent before the turn counts as over.
    quiet: Duration,

    /// How long to wait on a turn that reports no result.
    limit: Duration,
}

/// Reach the installed adapter, with its runtime behind `proxy` and a tool
/// server for it to call.
async fn live(prepared: &PreparedRequest, proxy: &Proxy, login: Option<&Utf8Path>) -> Probe {
    let mut launch = transport::launch(prepared, &context(), CachePolicy::Off, login)
        .expect("a prepared launch");

    // Set on the adapter and in the session options, as JP's own variables
    // are, and in both spellings, since Claude Code reads the lowercase one
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
            tap: cassette::tap(FIXTURE),
        }),
        environment: launch.environment,
        tools: tool_server().await,
        cut: proxy.cut.clone(),
        // Long enough to notice a runtime that starts another request after
        // reporting a result.
        quiet: Duration::from_secs(20),
        limit: Duration::from_mins(5),
    }
}

/// Answer from the recording.
///
/// Nothing connects to the tool server or the API during a replay, so the URL
/// names no listener and the cut has no tunnel to drop.
fn replayed(prepared: &PreparedRequest) -> Probe {
    Probe {
        transport: Box::new(Recorded(only_connection(FIXTURE))),
        environment: options::environment(prepared, CachePolicy::Off),
        tools: "http://127.0.0.1:1/mcp".to_owned(),
        cut: broadcast::channel(1).0,
        // A replay delivers the whole recording at once.
        quiet: Duration::from_millis(250),
        limit: Duration::from_secs(5),
    }
}

/// Enough of a text to recognise it by.
fn excerpt(text: &str) -> String {
    text.chars().take(120).collect()
}

/// What each content block of a message is, with the text of the ones that hold
/// any.
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

/// The outline entry for one SDK message, or `None` for a stream event that
/// only carries content.
///
/// Keeps what says how the turn is going and leaves out what differs between
/// two recordings of the same behaviour: identifiers, timestamps, usage, and
/// the deltas themselves.
/// A field the message does not have is left out of its entry.
fn entry(message: &Value) -> Option<Value> {
    let mut entry = match message["type"].as_str()? {
        "stream_event" => {
            let event = &message["event"];
            match event["type"].as_str()? {
                "message_start" | "message_stop" => json!({"stream": event["type"]}),
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
            }
        }
        "assistant" => json!({
            "assistant": blocks(&message["message"]["content"]),
            "error": message["error"],
            "aborted": message["aborted"],
        }),
        "user" => json!({"user": blocks(&message["message"]["content"])}),
        "result" => json!({
            "result": message["subtype"],
            "is_error": message["is_error"],
            "stop_reason": message["stop_reason"],
            "terminal_reason": message["terminal_reason"],
            "api_error_status": message["api_error_status"],
            "errors": message["errors"],
        }),
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

/// Whether this SDK message opens the content block of the [`STORE`] call.
fn opens_store_call(message: &Value) -> bool {
    let event = &message["event"];
    message["type"] == "stream_event"
        && event["type"] == "content_block_start"
        && event["content_block"]["name"].as_str() == Some(tool_name(STORE).as_str())
}

/// Outline everything the adapter sends, allow every tool call, and cut the
/// runtime's connections once, when the [`STORE`] call opens.
///
/// Unlike `transport::inbound`, an SDK message that reports a failure is
/// outlined and acknowledged like any other, so the connection outlives it.
///
/// `activity` is notified for every message and `finished` for every SDK
/// result.
fn observer(
    outline: Outline,
    activity: Arc<Notify>,
    finished: Arc<Notify>,
    cut: broadcast::Sender<()>,
) -> Handler {
    let was_cut = Arc::new(AtomicBool::new(false));

    Box::new(move |inbound| {
        let outline = outline.clone();
        let activity = activity.clone();
        let finished = finished.clone();
        let cut = cut.clone();
        let was_cut = was_cut.clone();

        Box::pin(async move {
            let (Inbound::Notification { method, params } | Inbound::Request { method, params }) =
                inbound;
            activity.notify_one();

            if method == SdkNotification::METHOD {
                let message = &params["message"];
                let mut entries = outline.lock().unwrap();
                entries.extend(entry(message));
                if opens_store_call(message) && !was_cut.swap(true, Ordering::Relaxed) {
                    // A replay has no tunnel listening, and takes the same
                    // path so its outline matches the recording's.
                    drop(cut.send(()));
                    entries.push(json!({"probe": "cut every connection to the API"}));
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

/// Send [`PROMPT`] in a fresh session and return the outline of the turn.
///
/// The session is set up the way `transport::drive` sets one up, less the
/// history.
/// The last entry says how the turn ended, since a runtime that has reported a
/// failure may never answer the prompt.
async fn observe(probe: Probe, prepared: PreparedRequest) -> Vec<Value> {
    let outline: Outline = Arc::default();
    let activity = Arc::new(Notify::new());
    let finished = Arc::new(Notify::new());
    let handler = observer(
        outline.clone(),
        activity.clone(),
        finished.clone(),
        probe.cut,
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

                // Each ending waits for silence before it is recorded, so
                // whatever trails it is in the outline first.
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

/// The snapshot is the finding: read the entries that follow the assistant
/// message carrying an `error`.
/// A `message_start` there means the runtime sent another request by itself,
/// and the `result` says whether the turn it belongs to succeeded.
///
/// Recording spends a little subscription allowance and takes about a minute:
///
/// ```sh
/// RECORD=1 cargo test -p jp_llm turn_after_a_cut_off_response -- --nocapture
/// ```
#[tokio::test]
async fn turn_after_a_cut_off_response() {
    let model = model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query()).unwrap();

    let turn = if cassette::recording() {
        // Reaching the runtime with a stale adapter or a lapsed login produces
        // a failure that reads as a protocol problem, so check first.
        let login = recording_login();
        inspect(login.as_deref())
            .await
            .expect("a qualified adapter and an active login");

        let proxy = Proxy::start().await;
        let turn = observe(live(&prepared, &proxy, login.as_deref()).await, prepared).await;

        // Printed before anything is asserted, so a recording that misses its
        // subject still shows what happened instead.
        println!("{:#}", json!(turn));
        println!("{:#}", json!(*proxy.log.lock().unwrap()));
        assert!(
            proxy.tunnels() > 0,
            "Claude Code never reached the API through the proxy, so no connection was cut"
        );
        turn
    } else {
        observe(replayed(&prepared), prepared).await
    };

    // The subject is a cut after one call completed. A login that fails, or a
    // model that orders the calls differently, also ends in an error, and
    // says nothing about what follows a cut.
    let position = |found: &dyn Fn(&Value) -> bool| turn.iter().position(found);
    let lookup = position(&|entry| entry["permission"] == json!(tool_name(LOOKUP)));
    let cut =
        position(&|entry| entry.get("probe") == Some(&json!("cut every connection to the API")));
    let error = position(&|entry| entry.get("assistant").is_some() && entry.get("error").is_some());
    assert!(
        matches!((lookup, cut, error), (Some(lookup), Some(cut), Some(error)) if lookup < cut && cut < error),
        "expected `lookup` to be dispatched, then the cut, then an assistant message carrying an \
         error; got them at {lookup:?}, {cut:?}, {error:?}: {turn:#?}"
    );

    insta::assert_json_snapshot!("turn_after_a_cut_off_response", turn);
}

/// The recorded cut, replayed through JP's own translation of the adapter.
///
/// Claude Code reports the lost connection and then finishes the turn itself:
/// it runs the call that completed before the cut, asks the model again, and
/// the model issues the interrupted call anew.
/// JP passes through that report the way the API path passes through a dropped
/// connection it recovers from, so the request ends as an ordinary completed
/// one, with each call handed to JP exactly once.
#[tokio::test]
async fn a_turn_claude_code_continues_after_a_cut_completes() {
    let model = model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query()).unwrap();
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
        NativeArtifact {
            session: None,
            path: None,
        },
        Box::new(Recorded(only_connection(FIXTURE))),
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
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
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
