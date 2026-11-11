//! Client for the `jp` host plugin protocol over stdin and stdout.
//!
//! [`Host`] defines the operations the bridge uses: starting a turn, sending an
//! in-turn reply, stopping a turn, reading a conversation's events and the
//! configured tools, and logging.
//! [`HostClient`] implements [`Host`] over JSON-lines stdio; tests use a fake
//! implementation instead.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io::{BufRead, Write},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use jp_plugin::message::{
    ExitMessage, HostToPlugin, InterruptRequest, LogMessage, PluginToHost, PrintMessage,
    QueryRequest, ReadConfigRequest, ReadEventsRequest,
};
use serde_json::{Map, Value};
use tokio::sync::{oneshot, watch};

use crate::lock;

/// Timeout for non-turn host requests (`created` handshake and `read_events`).
const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

/// Future that resolves when a running turn completes on the host.
pub(crate) type Turn = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Parameters for starting a turn in a new conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewTurn {
    pub content: String,
    pub title: String,
    pub cfg: Vec<String>,
    pub expires_in: String,
}

/// Failure to deliver an in-turn reply to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplyError {
    /// The host rejected the reply because no turn was running on that
    /// conversation.
    /// The caller should queue the message for the next turn.
    Refused(String),

    /// The stdio connection to the host failed or returned an unexpected
    /// message.
    Broken(String),
}

/// Severity of a log line sent to the host, spelled as the protocol takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Level(&'static str);

impl Level {
    pub(crate) const INFO: Self = Self("info");
    pub(crate) const WARN: Self = Self("warn");
    pub(crate) const ERROR: Self = Self("error");
}

/// Host operations used by the bridge.
pub(crate) trait Host: Clone + Send + Sync + 'static {
    /// Create a new conversation and start its first turn.
    ///
    /// The outer future resolves with the conversation ID and a [`Turn`] future
    /// as soon as the host sends `created`; [`Turn`] resolves when the turn
    /// finishes.
    fn start(&self, turn: NewTurn) -> impl Future<Output = Result<(String, Turn), String>> + Send;

    /// Start a turn on an existing `conversation`, compacting it first when
    /// `compact` is set.
    ///
    /// The request reaches the host before this returns, so a reply sent after
    /// it reaches the host after it.
    fn query(&self, conversation: &str, content: &str, cfg: Vec<String>, compact: bool) -> Turn;

    /// Inject `content` into the running turn on `conversation` as a user
    /// reply.
    fn reply(
        &self,
        conversation: &str,
        content: &str,
    ) -> impl Future<Output = Result<(), ReplyError>> + Send + 'static;

    /// Stop the running turn on `conversation`, keeping what it produced.
    ///
    /// Resolves once the turn has acted on it, or with an error when no turn
    /// was left to reach.
    fn stop(&self, conversation: &str)
    -> impl Future<Output = Result<(), String>> + Send + 'static;

    fn events(&self, conversation: &str)
    -> impl Future<Output = Result<Vec<Value>, String>> + Send;

    /// Names of the tools the host's configuration defines, read from disk
    /// again.
    fn tools(&self) -> impl Future<Output = Result<Vec<String>, String>> + Send;

    fn log(&self, level: Level, message: &str);

    /// Print a line to the user's terminal regardless of log level.
    fn print(&self, text: &str);
}

/// Shared stdout, written one whole message at a time.
type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// Waiters per request id, in the order their replies are expected.
type Pending = Arc<Mutex<HashMap<String, VecDeque<oneshot::Sender<HostToPlugin>>>>>;

#[derive(Clone)]
pub(crate) struct HostClient {
    writer: Writer,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

impl HostClient {
    /// A client writing to `stdout`, which reads no answers until
    /// [`Self::listen`] is called.
    pub(crate) fn new(stdout: impl Write + Send + 'static) -> Self {
        Self {
            writer: Arc::new(Mutex::new(Box::new(stdout))),
            pending: Arc::default(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Read the host's messages from `stdin` on a thread of its own, and hand
    /// each answer to the request waiting for it.
    ///
    /// The returned receiver becomes `true` when the host sends `shutdown` or
    /// closes stdin.
    pub(crate) fn listen(
        &self,
        stdin: impl BufRead + Send + 'static,
    ) -> Result<watch::Receiver<bool>, String> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let pending = Arc::clone(&self.pending);
        thread::Builder::new()
            .name("host-reader".into())
            .spawn(move || read_host(stdin, &pending, &shutdown_tx))
            .map_err(|error| format!("failed to start the host reader: {error}"))?;

        Ok(shutdown_rx)
    }

    /// Send an `exit` message to the host.
    pub(crate) fn exit(&self, code: u8, reason: Option<String>) -> Result<(), String> {
        self.send(&PluginToHost::Exit(ExitMessage {
            code,
            reason,
            expected: false,
        }))
    }

    fn next_id(&self) -> String {
        self.next_id.fetch_add(1, Ordering::Relaxed).to_string()
    }

    fn register(&self, id: &str) -> oneshot::Receiver<HostToPlugin> {
        let (tx, rx) = oneshot::channel();
        lock(&self.pending)
            .entry(id.to_owned())
            .or_default()
            .push_back(tx);
        rx
    }

    fn forget(&self, id: &str) {
        lock(&self.pending).remove(id);
    }

    /// Write `message` to the host as one line.
    pub(crate) fn send(&self, message: &PluginToHost) -> Result<(), String> {
        let json = serde_json::to_string(message).map_err(|error| error.to_string())?;
        let mut writer = lock(&self.writer);
        writeln!(writer, "{json}")
            .and_then(|()| writer.flush())
            .map_err(|error| format!("failed to write to the host: {error}"))
    }

    /// Send `message` and wait for its single response, with an optional
    /// `timeout`.
    async fn request(
        &self,
        id: &str,
        message: &PluginToHost,
        timeout: Option<Duration>,
    ) -> Result<HostToPlugin, String> {
        let rx = self.register(id);
        if let Err(error) = self.send(message) {
            self.forget(id);
            return Err(error);
        }

        let answer = match timeout {
            None => rx.await.map_err(|_| closed()),
            Some(timeout) => tokio::time::timeout(timeout, rx)
                .await
                .map_err(|_| "timed out waiting for the host".to_owned())
                .and_then(|answer| answer.map_err(|_| closed())),
        };
        if answer.is_err() {
            self.forget(id);
        }

        answer
    }
}

fn closed() -> String {
    "the connection to the host closed".to_owned()
}

/// Map a final query response message to `Ok(())` or an error string.
fn finished(answer: Result<HostToPlugin, String>) -> Result<(), String> {
    match answer? {
        HostToPlugin::QueryComplete(_) => Ok(()),
        HostToPlugin::Error(error) => Err(error.message),
        other => Err(format!("unexpected answer to a query: {other:?}")),
    }
}

impl Host for HostClient {
    fn start(&self, turn: NewTurn) -> impl Future<Output = Result<(String, Turn), String>> + Send {
        let client = self.clone();

        async move {
            let id = client.next_id();
            let message = PluginToHost::Query(QueryRequest {
                id: Some(id.clone()),
                conversation: String::new(),
                content: turn.content,
                new: true,
                title: Some(turn.title),
                cfg: turn.cfg,
                schema: None,
                expires_in: Some(turn.expires_in),
                compact: false,
            });

            // Register both waiters before sending the request so a fast turn cannot
            // deliver `created` and `query_complete` before the receivers are queued.
            let created = client.register(&id);
            let ended = client.register(&id);
            if let Err(error) = client.send(&message) {
                client.forget(&id);
                return Err(error);
            }

            let conversation = match tokio::time::timeout(REQUEST_TIMEOUT, created).await {
                Ok(Ok(HostToPlugin::Created(created))) => created.conversation,
                Ok(Ok(HostToPlugin::Error(error))) => {
                    client.forget(&id);
                    return Err(error.message);
                }
                Ok(Ok(other)) => {
                    client.forget(&id);
                    return Err(format!("unexpected answer to a new query: {other:?}"));
                }
                Ok(Err(_)) => return Err(closed()),
                Err(_) => {
                    client.forget(&id);
                    return Err("timed out waiting for the conversation".to_owned());
                }
            };

            let turn: Turn = Box::pin(async move { finished(ended.await.map_err(|_| closed())) });
            Ok((conversation, turn))
        }
    }

    fn query(&self, conversation: &str, content: &str, cfg: Vec<String>, compact: bool) -> Turn {
        let id = self.next_id();
        let message = PluginToHost::Query(QueryRequest {
            id: Some(id.clone()),
            conversation: conversation.to_owned(),
            content: content.to_owned(),
            new: false,
            title: None,
            cfg,
            schema: None,
            expires_in: None,
            compact,
        });

        // Written now rather than when the turn is first awaited, so it precedes
        // any reply the bridge sends once it knows the turn is running.
        let rx = self.register(&id);
        if let Err(error) = self.send(&message) {
            self.forget(&id);
            return Box::pin(async move { Err(error) });
        }

        // No deadline: a turn takes as long as the assistant does.
        Box::pin(async move { finished(rx.await.map_err(|_| closed())) })
    }

    fn reply(
        &self,
        conversation: &str,
        content: &str,
    ) -> impl Future<Output = Result<(), ReplyError>> + Send + 'static {
        let client = self.clone();
        let id = client.next_id();
        let message = PluginToHost::Interrupt(
            InterruptRequest::reply(conversation.to_owned(), content.to_owned())
                .with_id(id.clone()),
        );

        // No timeout: the host replies only after the turn processes the interrupt.
        async move {
            match client.request(&id, &message, None).await {
                Ok(HostToPlugin::Done(_)) => Ok(()),
                Ok(HostToPlugin::Error(error)) => Err(ReplyError::Refused(error.message)),
                Ok(other) => Err(ReplyError::Broken(format!(
                    "unexpected answer to a reply: {other:?}"
                ))),
                Err(error) => Err(ReplyError::Broken(error)),
            }
        }
    }

    fn stop(
        &self,
        conversation: &str,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        let client = self.clone();
        let id = client.next_id();
        let message = PluginToHost::Interrupt(
            InterruptRequest::stop(conversation.to_owned()).with_id(id.clone()),
        );

        async move {
            match client.request(&id, &message, None).await? {
                HostToPlugin::Done(_) => Ok(()),
                HostToPlugin::Error(error) => Err(error.message),
                other => Err(format!("unexpected answer to a stop: {other:?}")),
            }
        }
    }

    fn tools(&self) -> impl Future<Output = Result<Vec<String>, String>> + Send {
        let client = self.clone();

        async move {
            let id = client.next_id();
            let message = PluginToHost::ReadConfig(ReadConfigRequest {
                id: Some(id.clone()),
                path: Some("conversation".to_owned()),
                reload: true,
            });

            match client.request(&id, &message, Some(REQUEST_TIMEOUT)).await? {
                HostToPlugin::Config(config) => tool_names(config.data.get("tools")),
                HostToPlugin::Error(error) => Err(error.message),
                other => Err(format!("unexpected answer to read_config: {other:?}")),
            }
        }
    }

    fn events(
        &self,
        conversation: &str,
    ) -> impl Future<Output = Result<Vec<Value>, String>> + Send {
        let client = self.clone();
        let conversation = conversation.to_owned();

        async move {
            let id = client.next_id();
            let message = PluginToHost::ReadEvents(ReadEventsRequest {
                id: Some(id.clone()),
                conversation,
            });

            match client.request(&id, &message, Some(REQUEST_TIMEOUT)).await? {
                HostToPlugin::Events(events) => Ok(events.data),
                HostToPlugin::Error(error) => Err(error.message),
                other => Err(format!("unexpected answer to read_events: {other:?}")),
            }
        }
    }

    fn log(&self, level: Level, message: &str) {
        drop(self.send(&PluginToHost::Log(LogMessage {
            level: level.0.to_owned(),
            message: message.to_owned(),
            fields: Map::new(),
        })));
    }

    fn print(&self, text: &str) {
        drop(self.send(&PluginToHost::Print(PrintMessage {
            text: format!("{text}\n"),
            channel: "content".to_owned(),
            format: "plain".to_owned(),
            language: None,
        })));
    }
}

/// The tool names in a serialized `conversation.tools` table.
///
/// The table holds the `*` defaults beside one entry per tool, or, for a map
/// that states its merge strategy, the entries under `value`.
/// No table means no tools; anything else is an error rather than an empty
/// list, since a caller disables the tools it names.
pub(crate) fn tool_names(tools: Option<&Value>) -> Result<Vec<String>, String> {
    let table = match tools {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::Object(table)) => table,
        Some(other) => return Err(format!("`conversation.tools` is not a table: {other}")),
    };
    let entries = match table.get("value") {
        Some(Value::Object(value)) if table.contains_key("strategy") => value,
        _ => table,
    };

    Ok(entries
        .keys()
        .filter(|name| *name != "*")
        .cloned()
        .collect())
}

/// Read JSON-lines messages from the host and dispatch responses to waiting
/// callers.
fn read_host(reader: impl BufRead, pending: &Pending, shutdown: &watch::Sender<bool>) {
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<HostToPlugin>(&line) else {
            continue;
        };

        let id = match &message {
            HostToPlugin::Shutdown => {
                let _ = shutdown.send(true);
                continue;
            }
            HostToPlugin::Events(answer) => answer.id.clone(),
            HostToPlugin::Config(answer) => answer.id.clone(),
            HostToPlugin::QueryComplete(answer) => answer.id.clone(),
            HostToPlugin::Created(answer) => answer.id.clone(),
            HostToPlugin::Done(answer) => answer.id.clone(),
            HostToPlugin::Error(answer) => answer.id.clone(),
            _ => None,
        };
        let Some(id) = id else { continue };

        let waiter = {
            let mut pending = lock(pending);
            let waiter = pending.get_mut(&id).and_then(VecDeque::pop_front);
            if pending.get(&id).is_some_and(VecDeque::is_empty) {
                pending.remove(&id);
            }
            waiter
        };
        if let Some(waiter) = waiter {
            drop(waiter.send(message));
        }
    }

    // The host is gone: every waiter resolves as closed rather than hanging.
    lock(pending).clear();
    let _ = shutdown.send(true);
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
