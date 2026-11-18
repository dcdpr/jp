//! Multiplexed async host client for long-running command plugins.
//!
//! [`Client`] writes requests to the host on stdout and correlates responses by
//! request ID.
//! [`Client::listen`] spawns a reader thread on stdin that dispatches each
//! response to its waiting [`Answer`] channel and signals when the host sends
//! `shutdown` or closes the pipe.
//! Cloning a `Client` shares the underlying connection.
//!
//! Most requests expect a single response ([`Client::request`]).
//! A `query` with `new = true` receives two responses: `created` once the
//! conversation is created and locked, followed by `query_complete` when the
//! turn finishes ([`Client::send_expecting`]).
//!
//! [`Client`] returns [`HostToPlugin`] responses as sent by the host, including
//! [`HostToPlugin::Error`], and implements [`Log`] to forward structured log
//! events over the same stdout pipe.

use std::{
    array,
    collections::{HashMap, VecDeque},
    io::{self, BufRead, Write},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::Value;
use tokio::{
    sync::{oneshot, watch},
    time,
};

use crate::{
    log::{Level, Log, fields},
    message::{HostToPlugin, PluginToHost, PrintMessage},
};

/// Shared stdout writer for plugin-to-host messages.
///
/// Each JSON line is written and flushed while holding the mutex so concurrent
/// tasks and [`LogLayer`] never interleave partial lines.
///
/// [`LogLayer`]: crate::log::LogLayer
pub type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// Pending response channels keyed by request ID, in the order responses are
/// expected.
type Pending = Mutex<HashMap<String, VecDeque<oneshot::Sender<HostToPlugin>>>>;

/// Multiplexed connection to the `jp` host, shared across clones.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    writer: Writer,
    pending: Pending,
    next_id: AtomicU64,
}

/// Handle for awaiting a single [`HostToPlugin`] response via [`Answer::wait`].
#[derive(Debug)]
pub struct Answer {
    rx: oneshot::Receiver<HostToPlugin>,
}

/// Error returned when a host request fails before a response is received.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Writing or flushing the request to stdout failed.
    #[error("failed to write to the host: {0}")]
    Write(io::Error),

    /// Spawning the stdin reader thread failed.
    #[error("failed to start the host reader: {0}")]
    Listen(io::Error),

    /// The request has no correlation ID (`id` is `None`).
    #[error("the request carries no id to match its answer by")]
    NoId,

    /// The host closed the pipe before responding, or the request was cancelled
    /// with [`Client::forget`].
    #[error("the connection to the host closed")]
    Closed,

    /// The timeout elapsed before the host responded.
    #[error("timed out waiting for the host")]
    TimedOut,
}

impl Client {
    /// Create a client that writes requests to `writer`.
    ///
    /// Responses are not read until [`Client::listen`] is called, allowing the
    /// plugin to complete the `init`/`ready` handshake on stdin first.
    #[must_use]
    pub fn new(writer: Writer) -> Self {
        Self {
            inner: Arc::new(Inner {
                writer,
                pending: Mutex::default(),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    /// Spawn a background thread that reads [`HostToPlugin`] messages from
    /// `stdin` and dispatches responses to waiting [`Answer`]s.
    ///
    /// The returned watch receiver becomes `true` when the host sends
    /// `shutdown` or closes `stdin`.
    /// When `stdin` closes, all pending requests fail with [`Error::Closed`].
    /// Malformed lines and uncorrelated responses are logged and ignored.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Listen`] if the reader thread cannot be spawned.
    pub fn listen(
        &self,
        stdin: impl BufRead + Send + 'static,
    ) -> Result<watch::Receiver<bool>, Error> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let client = self.clone();
        thread::Builder::new()
            .name("host-reader".to_owned())
            .spawn(move || read(stdin, &client, &shutdown_tx))
            .map_err(Error::Listen)?;

        Ok(shutdown_rx)
    }

    /// Return the shared stdout [`Writer`], for passing to
    /// [`LogHandle::activate`].
    ///
    /// [`LogHandle::activate`]: crate::log::LogHandle::activate
    #[must_use]
    pub fn writer(&self) -> Writer {
        Arc::clone(&self.inner.writer)
    }

    /// Allocate a monotonically increasing correlation ID (`"1"`, `"2"`, ...).
    #[must_use]
    pub fn next_id(&self) -> String {
        self.inner
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string()
    }

    /// Serialize `message` as a single JSON line and flush it to the host.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Write`] if serialization or writing fails.
    pub fn send(&self, message: &PluginToHost) -> Result<(), Error> {
        let json = serde_json::to_string(message).map_err(|error| Error::Write(error.into()))?;
        let mut writer = lock(&self.inner.writer);

        writeln!(writer, "{json}")
            .and_then(|()| writer.flush())
            .map_err(Error::Write)
    }

    /// Send a plain-text `print` message on `channel` (`"content"` for stdout
    /// or `"chrome"` for stderr output suppressed by `--quiet`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Write`] if writing to the host fails.
    pub fn print(&self, text: &str, channel: &str) -> Result<(), Error> {
        self.send(&PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: channel.to_owned(),
            format: "plain".into(),
            language: None,
        }))
    }

    /// Send `message` and wait for a single response, with an optional
    /// `timeout`.
    ///
    /// Removes the pending waiter if waiting fails or times out.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send_expecting`] or [`Answer::wait`].
    pub async fn request(
        &self,
        message: &PluginToHost,
        timeout: Option<Duration>,
    ) -> Result<HostToPlugin, Error> {
        let [answer] = self.send_expecting(message)?;
        let answered = answer.wait(timeout).await;
        if answered.is_err()
            && let Some(id) = message.id()
        {
            self.forget(id);
        }

        answered
    }

    /// Send `message` expecting `N` responses on its correlation ID, returning
    /// their [`Answer`] handles in order.
    ///
    /// Registers the response channels before writing the request so an
    /// immediate response cannot arrive before its waiter is registered.
    /// Cleans up the channels if sending fails.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoId`] if `message` has no correlation ID, or
    /// [`Error::Write`] if writing to the host fails.
    pub fn send_expecting<const N: usize>(
        &self,
        message: &PluginToHost,
    ) -> Result<[Answer; N], Error> {
        let id = message.id().ok_or(Error::NoId)?;
        let answers = array::from_fn(|_| self.wait_for(id));
        if let Err(error) = self.send(message) {
            self.forget(id);
            return Err(error);
        }

        Ok(answers)
    }

    /// Cancel all pending [`Answer`]s for request `id`.
    ///
    /// Any task awaiting those [`Answer`]s receives [`Error::Closed`], and any
    /// subsequent host response for `id` is logged and ignored.
    pub fn forget(&self, id: &str) {
        lock(&self.inner.pending).remove(id);
    }

    /// Register a oneshot channel for the next response to request `id`.
    fn wait_for(&self, id: &str) -> Answer {
        let (tx, rx) = oneshot::channel();
        lock(&self.inner.pending)
            .entry(id.to_owned())
            .or_default()
            .push_back(tx);

        Answer { rx }
    }
}

impl Answer {
    /// Wait for the host's response, with an optional `timeout`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TimedOut`] if `timeout` elapses first, or
    /// [`Error::Closed`] if the connection closes or [`Client::forget`] cancels
    /// the request.
    pub async fn wait(self, timeout: Option<Duration>) -> Result<HostToPlugin, Error> {
        let Some(timeout) = timeout else {
            return self.rx.await.map_err(|_| Error::Closed);
        };

        time::timeout(timeout, self.rx)
            .await
            .map_err(|_| Error::TimedOut)?
            .map_err(|_| Error::Closed)
    }
}

/// Read [`HostToPlugin`] lines from `stdin` until EOF, dispatching responses to
/// pending waiters and signalling `shutdown` on `Shutdown` or EOF.
fn read(stdin: impl BufRead, client: &Client, shutdown: &watch::Sender<bool>) {
    for line in stdin.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                client.log_with(
                    Level::ERROR,
                    "Failed to read from the host.",
                    fields([("error", error.to_string().into())]),
                );
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let message: HostToPlugin = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                client.log_with(
                    Level::WARN,
                    "Received a line from the host that isn't a message.",
                    fields([("error", error.to_string().into()), ("line", line.into())]),
                );
                continue;
            }
        };

        match message {
            HostToPlugin::Shutdown => {
                let _ = shutdown.send(true);
            }
            HostToPlugin::Init(_) | HostToPlugin::Describe => client.log_with(
                Level::WARN,
                "Received a startup message after startup.",
                fields([("type", kind(&message).into())]),
            ),
            answer => dispatch(client, answer),
        }
    }

    // Clear all pending waiters on EOF so awaiting tasks fail immediately with
    // `Error::Closed` instead of hanging.
    lock(&client.inner.pending).clear();
    let _ = shutdown.send(true);
}

/// Deliver `answer` to the next waiter registered for its correlation ID,
/// removing the entry once its queue is empty.
fn dispatch(client: &Client, answer: HostToPlugin) {
    let Some(id) = answer.id().map(ToOwned::to_owned) else {
        client.log_with(
            Level::WARN,
            "Received an answer that names no request.",
            fields([("type", kind(&answer).into())]),
        );
        return;
    };

    let waiter = {
        let mut pending = lock(&client.inner.pending);
        let waiter = pending.get_mut(&id).and_then(VecDeque::pop_front);
        if pending.get(&id).is_some_and(VecDeque::is_empty) {
            pending.remove(&id);
        }
        waiter
    };

    let Some(waiter) = waiter else {
        client.log_with(
            Level::WARN,
            "Received an answer no request waits for.",
            fields([("id", id.into()), ("type", kind(&answer).into())]),
        );
        return;
    };

    // Ignore send errors when the caller has already dropped its `Answer`.
    drop(waiter.send(answer));
}

/// Extract the wire `type` tag of `message` (e.g. `"done"`).
fn kind(message: &HostToPlugin) -> String {
    serde_json::to_value(message)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default()
}

/// Lock `mutex`, recovering the inner value if a previous holder panicked.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
