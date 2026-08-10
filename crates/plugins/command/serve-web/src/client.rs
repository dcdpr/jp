//! Protocol client for communicating with the JP host.
//!
//! Manages the stdin reader loop and provides async methods for sending
//! requests and awaiting responses.
//! Thread-safe and shareable across axum handlers via `Arc`.

use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use jp_plugin::message::{
    ConfigEntry, ConversationRequest, ConversationSummary, DraftResponse, EventsResponse,
    ExitMessage, HostToPlugin, InterruptRequest, OptionalId, PluginToHost, QueryRequest,
    ReadEventsRequest, SetTitleRequest, WriteDraftRequest,
};
use tokio::sync::{oneshot, watch};
use tracing::{debug, error, trace, warn};

/// Shared writer for stdout, used by both the protocol client and the tracing
/// log layer.
pub type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// How long a request waits for a matching host response before giving up.
///
/// Guards against a lost or id-less response leaving a handler awaiting
/// forever, which would otherwise stall graceful shutdown.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a delegated turn is given before the request is abandoned.
///
/// A turn runs the whole agent loop: the model thinks, tools run, the model
/// thinks again.
/// Minutes are normal, so this is generous — it exists to stop a lost response
/// from pinning a browser connection open forever, not to bound how long the
/// assistant may take.
const QUERY_TIMEOUT: Duration = Duration::from_mins(15);

/// A protocol client that talks to the JP host over stdin/stdout.
///
/// Cloneable via `Arc` internally — pass it into axum state directly.
#[derive(Clone)]
pub struct PluginClient {
    inner: Arc<Inner>,
}

/// The still-running turn a newly created conversation was started with.
///
/// Held by whoever needs to know when that turn ends — which is not the
/// request that created the conversation, since it returned as soon as there
/// was somewhere to send the reader.
pub struct TurnOutcome {
    rx: oneshot::Receiver<HostToPlugin>,
}

impl TurnOutcome {
    /// Wait for the turn to finish.
    ///
    /// Takes as long as the turn does, which can be minutes.
    pub async fn finished(self) -> Result<(), ClientError> {
        match tokio::time::timeout(QUERY_TIMEOUT, self.rx).await {
            Ok(Ok(HostToPlugin::QueryComplete(_))) => Ok(()),
            Ok(Ok(HostToPlugin::Error(e))) => Err(ClientError::Host(e.message)),
            Ok(Ok(other)) => Err(ClientError::Unexpected(format!("{other:?}"))),
            Ok(Err(_)) => Err(ClientError::ChannelClosed),
            Err(_) => Err(ClientError::Timeout),
        }
    }
}

struct Inner {
    writer: SharedWriter,

    /// Waiters per request, in the order their replies are expected.
    ///
    /// A queue rather than one waiter, because a request can be answered more
    /// than once: starting a conversation is told the id as soon as it exists
    /// and told again when its first turn ends.
    /// Both waiters are registered before the request goes out, so a turn that
    /// finishes quickly cannot arrive before anything is listening for it.
    pending: Mutex<HashMap<String, VecDeque<oneshot::Sender<HostToPlugin>>>>,
    next_id: AtomicU64,
}

impl PluginClient {
    /// Start the protocol client.
    ///
    /// Spawns a background thread that reads from `stdin` and dispatches
    /// responses to pending requests.
    /// Returns the client and a watch channel that signals when a shutdown
    /// message is received from the host.
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
        let msg = PluginToHost::ListConversations(OptionalId {
            id: Some(id.clone()),
        });

        match self.request(&id, &msg).await? {
            HostToPlugin::Conversations(resp) => Ok(resp.data),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Request events for a specific conversation.
    pub async fn read_events(&self, conversation: &str) -> Result<EventsResponse, ClientError> {
        let id = self.next_id();
        let msg = PluginToHost::ReadEvents(ReadEventsRequest {
            id: Some(id.clone()),
            conversation: conversation.to_owned(),
        });

        match self.request(&id, &msg).await? {
            HostToPlugin::Events(resp) => Ok(resp),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Ask the host to run a turn on a conversation.
    ///
    /// Returns once the turn has finished and its events are persisted; read
    /// them back with [`Self::read_events`].
    /// The host owns the agent loop, so this resolves the model, calls the
    /// provider, and runs tools without the plugin seeing any of it.
    pub async fn query(
        &self,
        conversation: &str,
        content: &str,
        cfg: Vec<String>,
    ) -> Result<(), ClientError> {
        let id = self.next_id();
        let msg = PluginToHost::Query(QueryRequest {
            new: false,
            title: None,
            cfg,
            id: Some(id.clone()),
            conversation: conversation.to_owned(),
            content: content.to_owned(),
        });

        match self.request_within(&id, &msg, QUERY_TIMEOUT).await? {
            HostToPlugin::QueryComplete(_) => Ok(()),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// List the configurations a new conversation can be started with.
    pub async fn list_configs(&self) -> Result<Vec<ConfigEntry>, ClientError> {
        let id = self.next_id();
        let msg = PluginToHost::ListConfigs(OptionalId {
            id: Some(id.clone()),
        });

        match self.request(&id, &msg).await? {
            HostToPlugin::Configs(resp) => Ok(resp.data),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Start a conversation and set its first turn running.
    ///
    /// Returns the id the host gave it, which is the only place that id exists:
    /// the conversation did not exist when the request was sent.
    ///
    /// Returns as soon as the conversation exists, not when the turn finishes.
    /// The turn's progress is in the conversation's events, which is where a
    /// reader sent to it will be looking anyway.
    pub async fn start_conversation(
        &self,
        content: &str,
        title: Option<String>,
        cfg: Vec<String>,
    ) -> Result<(String, TurnOutcome), ClientError> {
        let id = self.next_id();
        let msg = PluginToHost::Query(QueryRequest {
            id: Some(id.clone()),
            conversation: String::new(),
            content: content.to_owned(),
            new: true,
            title,
            cfg,
        });

        // Both waiters before the request goes out. Registering the second one
        // after the first reply arrives would race a turn that finished in between,
        // and a lost completion leaves the conversation marked busy forever.
        let created = self.register(&id);
        let finished = self.register(&id);

        if let Err(error) = self.send(&msg) {
            self.forget(&id);
            return Err(error);
        }

        // The default timeout, not the turn's: the host answers as soon as the
        // conversation exists, without waiting for its first turn.
        let conversation = match tokio::time::timeout(REQUEST_TIMEOUT, created).await {
            Ok(Ok(HostToPlugin::Created(resp))) => resp.conversation,
            Ok(Ok(HostToPlugin::Error(e))) => {
                self.forget(&id);
                return Err(ClientError::Host(e.message));
            }
            Ok(Ok(other)) => {
                self.forget(&id);
                return Err(ClientError::Unexpected(format!("{other:?}")));
            }
            Ok(Err(_)) => {
                self.forget(&id);
                return Err(ClientError::ChannelClosed);
            }
            Err(_) => {
                self.forget(&id);
                return Err(ClientError::Timeout);
            }
        };

        Ok((conversation, TurnOutcome { rx: finished }))
    }

    /// Move a conversation to the archive.
    pub async fn archive(&self, conversation: &str) -> Result<(), ClientError> {
        let id = self.next_id();
        let msg = PluginToHost::ArchiveConversation(ConversationRequest {
            id: Some(id.clone()),
            conversation: conversation.to_owned(),
        });

        self.done(&id, &msg).await
    }

    /// Rename a conversation.
    /// An empty title clears it.
    pub async fn set_title(&self, conversation: &str, title: &str) -> Result<(), ClientError> {
        let id = self.next_id();
        let msg = PluginToHost::SetTitle(SetTitleRequest {
            id: Some(id.clone()),
            conversation: conversation.to_owned(),
            title: Some(title.to_owned()),
        });

        self.done(&id, &msg).await
    }

    /// Send a request whose only answer is whether it worked.
    async fn done(&self, id: &str, msg: &PluginToHost) -> Result<(), ClientError> {
        match self.request(id, msg).await? {
            HostToPlugin::Done(_) => Ok(()),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Read a conversation's query draft.
    pub async fn read_draft(&self, conversation: &str) -> Result<DraftResponse, ClientError> {
        let id = self.next_id();
        let msg = PluginToHost::ReadDraft(ConversationRequest {
            id: Some(id.clone()),
            conversation: conversation.to_owned(),
        });

        match self.request(&id, &msg).await? {
            HostToPlugin::Draft(resp) => Ok(resp),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Replace a conversation's query draft, if it still matches `revision`.
    ///
    /// A refusal comes back as a [`DraftResponse`] with `conflict` set and the
    /// current draft attached, rather than as an error: the caller needs the
    /// other side's text to do anything sensible about it.
    pub async fn write_draft(
        &self,
        conversation: &str,
        content: &str,
        revision: Option<String>,
    ) -> Result<DraftResponse, ClientError> {
        let id = self.next_id();
        let msg = PluginToHost::WriteDraft(WriteDraftRequest {
            id: Some(id.clone()),
            conversation: conversation.to_owned(),
            content: content.to_owned(),
            revision,
        });

        match self.request(&id, &msg).await? {
            HostToPlugin::Draft(resp) => Ok(resp),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Ask the host to interrupt the turn it is running.
    ///
    /// Returns as soon as the request is on the wire.
    /// There is no reply to wait for: the interrupt lands in the conversation,
    /// and the turn's own outcome still arrives as the answer to the `query`
    /// that started it.
    pub fn interrupt(&self, conversation: &str) -> Result<(), ClientError> {
        self.send(&PluginToHost::Interrupt(InterruptRequest {
            conversation: conversation.to_owned(),
        }))
    }

    /// Register a request, send it, and await the matching response.
    ///
    /// Removes the pending entry on a transport failure (send error or timeout)
    /// so a stalled or failed request can't leak a dead sender.
    /// A delivered response is removed by `dispatch`, and a closed channel
    /// leaves nothing to remove, so the cleanup here targets only the
    /// transport-error paths.
    async fn request(&self, id: &str, msg: &PluginToHost) -> Result<HostToPlugin, ClientError> {
        self.request_within(id, msg, REQUEST_TIMEOUT).await
    }

    /// [`Self::request`], with a deadline of the caller's choosing.
    async fn request_within(
        &self,
        id: &str,
        msg: &PluginToHost,
        timeout: Duration,
    ) -> Result<HostToPlugin, ClientError> {
        let rx = self.register(id);

        let result = match self.send(msg) {
            Ok(()) => await_response(rx, timeout).await,
            Err(e) => Err(e),
        };

        if result.is_err() {
            self.inner
                .pending
                .lock()
                .expect("pending lock poisoned")
                .remove(id);
        }

        result
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
            .entry(id.to_owned())
            .or_default()
            .push_back(tx);
        rx
    }

    /// Drop every waiter for a request that will never be answered again.
    fn forget(&self, id: &str) {
        self.inner
            .pending
            .lock()
            .expect("pending lock poisoned")
            .remove(id);
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
    /// No response arrived within [`REQUEST_TIMEOUT`].
    Timeout,
}

/// Await a pending response, failing with [`ClientError`] on a closed channel
/// or timeout instead of blocking forever.
async fn await_response(
    rx: oneshot::Receiver<HostToPlugin>,
    timeout: Duration,
) -> Result<HostToPlugin, ClientError> {
    tokio::time::timeout(timeout, rx)
        .await
        .map_err(|_| ClientError::Timeout)?
        .map_err(|_| ClientError::ChannelClosed)
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(msg) => write!(f, "host error: {msg}"),
            Self::Unexpected(msg) => write!(f, "unexpected response: {msg}"),
            Self::ChannelClosed => write!(f, "protocol channel closed"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::Timeout => write!(f, "timed out waiting for host response"),
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
            HostToPlugin::QueryComplete(r) => r.id.clone(),
            HostToPlugin::Configs(r) => r.id.clone(),
            HostToPlugin::Created(r) => r.id.clone(),
            HostToPlugin::Done(r) => r.id.clone(),
            HostToPlugin::Draft(r) => r.id.clone(),
            HostToPlugin::Error(r) => r.id.clone(),
            _ => None,
        };

        match msg {
            HostToPlugin::Shutdown => {
                debug!("Received shutdown from host");
                let _ = shutdown_tx.send(true);
            }

            // `Composed` answers a `Compose` request, which this plugin never
            // sends: it serves HTTP and has no prompts to raise.
            HostToPlugin::Init(_) | HostToPlugin::Describe | HostToPlugin::Composed(_) => {
                warn!("Unexpected message after startup");
            }

            // Response messages — dispatch to the pending request.
            msg @ (HostToPlugin::Conversations(_)
            | HostToPlugin::Events(_)
            | HostToPlugin::Config(_)
            | HostToPlugin::QueryComplete(_)
            | HostToPlugin::Configs(_)
            | HostToPlugin::Created(_)
            | HostToPlugin::Done(_)
            | HostToPlugin::Draft(_)
            | HostToPlugin::Error(_)) => {
                dispatch(&inner.pending, req_id.as_deref(), msg);
            }
        }
    }

    // stdin closed — host process is gone. Drop every pending sender so any
    // in-flight request resolves with `ChannelClosed` instead of hanging and
    // stalling graceful shutdown.
    inner.pending.lock().expect("pending lock poisoned").clear();

    debug!("stdin reader loop exited");
    let _ = shutdown_tx.send(true);
}

/// Dispatch a response to the pending request with the given ID.
fn dispatch(
    pending: &Mutex<HashMap<String, VecDeque<oneshot::Sender<HostToPlugin>>>>,
    id: Option<&str>,
    msg: HostToPlugin,
) {
    let Some(id) = id else {
        warn!("Response without ID, cannot dispatch: {msg:?}");
        return;
    };

    // Taken in order, and the entry removed once its last waiter is served, so an
    // id that expects one reply behaves exactly as it did before.
    let tx = {
        let mut pending = pending.lock().expect("pending lock poisoned");
        let tx = pending.get_mut(id).and_then(VecDeque::pop_front);
        if pending.get(id).is_some_and(VecDeque::is_empty) {
            pending.remove(id);
        }
        tx
    };

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
