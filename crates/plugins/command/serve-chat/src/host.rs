//! Client for the `jp` host plugin protocol over stdin and stdout.
//!
//! [`Host`] defines the operations the bridge uses: starting a turn, sending an
//! in-turn reply, stopping a turn, reading a conversation's events and the
//! configured tools, and logging, through the `Log` every plugin logs with.
//! [`HostClient`] implements [`Host`] over the protocol client `jp_plugin`
//! shares between plugins; tests use a fake implementation instead.

use std::{
    future::Future,
    io::{BufRead, Write},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use jp_plugin::{
    client::{Client, Error as ConnectionError},
    log::{Level, Log},
    message::{
        CompactRequest, Compaction, ExitMessage, HostToPlugin, InterruptRequest, PluginToHost,
        PrintMessage, QueryRequest, ReadConfigRequest, ReadEventsRequest, StoredCompaction,
    },
};
use serde_json::{Map, Value};
use tokio::sync::watch;

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

/// Host operations used by the bridge.
pub(crate) trait Host: Log + Clone + Send + Sync + 'static {
    /// Create a new conversation and start its first turn.
    ///
    /// The outer future resolves with the conversation ID and a [`Turn`] future
    /// as soon as the host sends `created`; [`Turn`] resolves when the turn
    /// finishes.
    fn start(&self, turn: NewTurn) -> impl Future<Output = Result<(String, Turn), String>> + Send;

    /// Start a turn on an existing `conversation`.
    ///
    /// Sends the `query` request synchronously before returning the [`Turn`]
    /// future.
    fn query(&self, conversation: &str, content: &str, cfg: Vec<String>) -> Turn;

    /// Compact `conversation` with `compactions` and return the stored
    /// compactions.
    fn compact(
        &self,
        conversation: &str,
        compactions: Vec<Compaction>,
    ) -> impl Future<Output = Result<Vec<StoredCompaction>, String>> + Send + 'static;

    /// Inject `content` into the running turn on `conversation` as a user
    /// reply.
    fn reply(
        &self,
        conversation: &str,
        content: &str,
    ) -> impl Future<Output = Result<(), ReplyError>> + Send + 'static;

    /// Stop the active turn on `conversation`, keeping the output produced so
    /// far.
    fn stop(&self, conversation: &str)
    -> impl Future<Output = Result<(), String>> + Send + 'static;

    fn events(&self, conversation: &str)
    -> impl Future<Output = Result<Vec<Value>, String>> + Send;

    /// Reload the host configuration from disk and return the configured tool
    /// names.
    fn tools(&self) -> impl Future<Output = Result<Vec<String>, String>> + Send;

    /// Print a line to the user's terminal regardless of log level.
    fn print(&self, text: &str);
}

#[derive(Clone)]
pub(crate) struct HostClient {
    connection: Client,
}

impl HostClient {
    /// Create a client that writes requests to `stdout`.
    /// Call [`Self::listen`] to start reading responses.
    pub(crate) fn new(stdout: impl Write + Send + 'static) -> Self {
        Self {
            connection: Client::new(Arc::new(Mutex::new(Box::new(stdout)))),
        }
    }

    /// Spawn a background thread to read host messages from `stdin` and
    /// dispatch responses to waiting callers.
    ///
    /// The returned receiver becomes `true` when the host sends `shutdown` or
    /// closes stdin.
    pub(crate) fn listen(
        &self,
        stdin: impl BufRead + Send + 'static,
    ) -> Result<watch::Receiver<bool>, String> {
        self.connection
            .listen(stdin)
            .map_err(|error| error.to_string())
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
        self.connection.next_id()
    }

    /// Write `message` to the host as one line.
    pub(crate) fn send(&self, message: &PluginToHost) -> Result<(), String> {
        self.connection
            .send(message)
            .map_err(|error| error.to_string())
    }

    /// Send `message` and wait for its single response, with an optional
    /// `timeout`.
    async fn request(
        &self,
        message: &PluginToHost,
        timeout: Option<Duration>,
    ) -> Result<HostToPlugin, String> {
        self.connection
            .request(message, timeout)
            .await
            .map_err(|error| error.to_string())
    }
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
            });

            // Wait for both answers before sending the request so a fast turn cannot
            // deliver `created` and `query_complete` before anything waits for them.
            let [created, ended] = client
                .connection
                .send_expecting(&message)
                .map_err(|error| error.to_string())?;

            let conversation = match created.wait(Some(REQUEST_TIMEOUT)).await {
                Ok(HostToPlugin::Created(created)) => created.conversation,
                answer => {
                    client.connection.forget(&id);
                    return Err(match answer {
                        Ok(HostToPlugin::Error(error)) => error.message,
                        Ok(other) => format!("unexpected answer to a new query: {other:?}"),
                        Err(ConnectionError::TimedOut) => {
                            "timed out waiting for the conversation".to_owned()
                        }
                        Err(error) => error.to_string(),
                    });
                }
            };

            let turn: Turn = Box::pin(async move {
                finished(ended.wait(None).await.map_err(|error| error.to_string()))
            });
            Ok((conversation, turn))
        }
    }

    fn query(&self, conversation: &str, content: &str, cfg: Vec<String>) -> Turn {
        let message = PluginToHost::Query(QueryRequest {
            id: Some(self.next_id()),
            conversation: conversation.to_owned(),
            content: content.to_owned(),
            new: false,
            title: None,
            cfg,
            schema: None,
            expires_in: None,
        });

        // Send the request immediately so any subsequent in-turn reply is ordered
        // after it.
        let [ended] = match self.connection.send_expecting(&message) {
            Ok(answers) => answers,
            Err(error) => {
                let error = error.to_string();
                return Box::pin(async move { Err(error) });
            }
        };

        // No deadline: a turn takes as long as the assistant does.
        Box::pin(async move { finished(ended.wait(None).await.map_err(|error| error.to_string())) })
    }

    fn reply(
        &self,
        conversation: &str,
        content: &str,
    ) -> impl Future<Output = Result<(), ReplyError>> + Send + 'static {
        let client = self.clone();
        let message = PluginToHost::Interrupt(
            InterruptRequest::reply(conversation.to_owned(), content.to_owned())
                .with_id(client.next_id()),
        );

        // No timeout: the host replies only after the turn processes the interrupt.
        async move {
            match client.request(&message, None).await {
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
        let message = PluginToHost::Interrupt(
            InterruptRequest::stop(conversation.to_owned()).with_id(client.next_id()),
        );

        async move {
            match client.request(&message, None).await? {
                HostToPlugin::Done(_) => Ok(()),
                HostToPlugin::Error(error) => Err(error.message),
                other => Err(format!("unexpected answer to a stop: {other:?}")),
            }
        }
    }

    fn compact(
        &self,
        conversation: &str,
        compactions: Vec<Compaction>,
    ) -> impl Future<Output = Result<Vec<StoredCompaction>, String>> + Send + 'static {
        let client = self.clone();
        let conversation = conversation.to_owned();

        async move {
            let message = PluginToHost::Compact(CompactRequest {
                id: Some(client.next_id()),
                conversation,
                compactions,
            });

            // No timeout: summary compactions call an LLM.
            match client.request(&message, None).await? {
                HostToPlugin::Compacted(compacted) => Ok(compacted.compactions),
                HostToPlugin::Error(error) => Err(error.message),
                other => Err(format!("unexpected answer to compact: {other:?}")),
            }
        }
    }

    fn tools(&self) -> impl Future<Output = Result<Vec<String>, String>> + Send {
        let client = self.clone();

        async move {
            let message = PluginToHost::ReadConfig(ReadConfigRequest {
                id: Some(client.next_id()),
                path: Some("conversation".to_owned()),
                reload: true,
            });

            match client.request(&message, Some(REQUEST_TIMEOUT)).await? {
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
            let message = PluginToHost::ReadEvents(ReadEventsRequest {
                id: Some(client.next_id()),
                conversation,
            });

            match client.request(&message, Some(REQUEST_TIMEOUT)).await? {
                HostToPlugin::Events(events) => Ok(events.data),
                HostToPlugin::Error(error) => Err(error.message),
                other => Err(format!("unexpected answer to read_events: {other:?}")),
            }
        }
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

impl Log for HostClient {
    fn log_with(&self, level: Level, message: &'static str, fields: Map<String, Value>) {
        self.connection.log_with(level, message, fields);
    }
}

/// Extract configured tool names from a serialized `conversation.tools` value.
///
/// Skips the `*` defaults entry and unwraps `{ value, strategy }` merge tables.
/// Returns an error if `tools` is present but not an object.
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

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
