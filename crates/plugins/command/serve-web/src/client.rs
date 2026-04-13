//! Protocol client for communicating with the JP host.
//!
//! Manages the stdin reader loop and provides async methods for sending
//! requests and awaiting responses. Thread-safe and shareable across axum
//! handlers via `Arc`.

use std::{
    collections::HashMap,
    io::{BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use jp_plugin::message::{
    ConversationSummary, EventsResponse, ExitMessage, HostToPlugin, OptionalId, PluginToHost,
    ReadEventsRequest,
};
use tokio::sync::{oneshot, watch};
use tracing::{debug, error, trace, warn};

/// Shared writer for stdout, used by both the protocol client and the
/// tracing log layer.
pub type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// A protocol client that talks to the JP host over stdin/stdout.
///
/// Cloneable via `Arc` internally — pass it into axum state directly.
#[derive(Clone)]
pub struct PluginClient {
    inner: Arc<Inner>,
}

struct Inner {
    writer: SharedWriter,
    pending: Mutex<HashMap<String, oneshot::Sender<HostToPlugin>>>,
    next_id: AtomicU64,
}

impl PluginClient {
    /// Start the protocol client.
    ///
    /// Spawns a background thread that reads from `stdin` and dispatches
    /// responses to pending requests. Returns the client and a watch channel
    /// that signals when a shutdown message is received from the host.
    pub fn start(
        stdin: impl BufRead + Send + 'static,
        writer: SharedWriter,
    ) -> (Self, watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let inner = Arc::new(Inner {
            writer,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        });

        let reader_inner = inner.clone();
        thread::Builder::new()
            .name("stdin-reader".into())
            .spawn(move || reader_loop(stdin, &reader_inner, &shutdown_tx))
            .expect("failed to spawn stdin reader thread");

        (Self { inner }, shutdown_rx)
    }

    /// Request the list of conversations from the host.
    pub async fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ClientError> {
        let id = self.next_id();
        let rx = self.register(&id);

        self.send(&PluginToHost::ListConversations(OptionalId {
            id: Some(id.clone()),
        }))?;

        match rx.await.map_err(|_| ClientError::ChannelClosed)? {
            HostToPlugin::Conversations(resp) => Ok(resp.data),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Request events for a specific conversation.
    pub async fn read_events(&self, conversation: &str) -> Result<EventsResponse, ClientError> {
        let id = self.next_id();
        let rx = self.register(&id);

        self.send(&PluginToHost::ReadEvents(ReadEventsRequest {
            id: Some(id.clone()),
            conversation: conversation.to_owned(),
        }))?;

        match rx.await.map_err(|_| ClientError::ChannelClosed)? {
            HostToPlugin::Events(resp) => Ok(resp),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Send an exit message to the host.
    pub fn send_exit(&self, code: u8) {
        drop(self.send(&PluginToHost::Exit(ExitMessage { code, reason: None })));
    }

    fn next_id(&self) -> String {
        self.inner
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string()
    }

    fn register(&self, id: &str) -> oneshot::Receiver<HostToPlugin> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .expect("pending lock poisoned")
            .insert(id.to_owned(), tx);
        rx
    }

    fn send(&self, msg: &PluginToHost) -> Result<(), ClientError> {
        let json = serde_json::to_string(msg).map_err(|e| ClientError::Protocol(e.to_string()))?;
        let mut writer = self.inner.writer.lock().expect("writer lock poisoned");
        writeln!(writer, "{json}").map_err(|e| ClientError::Protocol(e.to_string()))?;
        writer
            .flush()
            .map_err(|e| ClientError::Protocol(e.to_string()))
    }
}

/// Errors from the plugin client.
#[derive(Debug)]
pub enum ClientError {
    /// The host returned an error response.
    Host(String),
    /// Unexpected response type.
    Unexpected(String),
    /// The response channel was closed (reader thread died).
    ChannelClosed,
    /// Protocol-level I/O or serialization error.
    Protocol(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(msg) => write!(f, "host error: {msg}"),
            Self::Unexpected(msg) => write!(f, "unexpected response: {msg}"),
            Self::ChannelClosed => write!(f, "protocol channel closed"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
        }
    }
}

/// Background loop that reads stdin and dispatches messages.
fn reader_loop(reader: impl BufRead, inner: &Inner, shutdown_tx: &watch::Sender<bool>) {
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                error!("stdin read error: {e}");
                break;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let msg: HostToPlugin = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                warn!("invalid host message: {e}: {line}");
                continue;
            }
        };

        trace!(?msg, "Received host message");

        // Extract the request ID (if any) before moving `msg` into dispatch.
        let req_id = match &msg {
            HostToPlugin::Conversations(r) => r.id.clone(),
            HostToPlugin::Events(r) => r.id.clone(),
            HostToPlugin::Config(r) => r.id.clone(),
            HostToPlugin::Error(r) => r.id.clone(),
            _ => None,
        };

        match msg {
            HostToPlugin::Shutdown => {
                debug!("Received shutdown from host");
                let _ = shutdown_tx.send(true);
            }

            HostToPlugin::Init(_) | HostToPlugin::Describe => {
                warn!("Unexpected message after startup");
            }

            // Response messages — dispatch to the pending request.
            msg @ (HostToPlugin::Conversations(_)
            | HostToPlugin::Events(_)
            | HostToPlugin::Config(_)
            | HostToPlugin::Error(_)) => {
                dispatch(&inner.pending, req_id.as_deref(), msg);
            }
        }
    }

    // stdin closed — host process is gone.
    debug!("stdin reader loop exited");
    let _ = shutdown_tx.send(true);
}

/// Dispatch a response to the pending request with the given ID.
fn dispatch(
    pending: &Mutex<HashMap<String, oneshot::Sender<HostToPlugin>>>,
    id: Option<&str>,
    msg: HostToPlugin,
) {
    let Some(id) = id else {
        warn!("Response without ID, cannot dispatch: {msg:?}");
        return;
    };

    let tx = pending.lock().expect("pending lock poisoned").remove(id);

    match tx {
        Some(tx) => {
            drop(tx.send(msg));
        }
        None => {
            warn!("No pending request for ID {id}");
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
