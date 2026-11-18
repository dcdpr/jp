//! The plugin's end of a connection to the host that lasts as long as the
//! plugin runs, for a plugin such as a server that keeps asking `jp` for
//! things.
//!
//! [`Client`] writes requests to the host and matches the host's answers to
//! them by request id.
//! A thread of its own reads the host's messages ([`Client::listen`]), hands
//! each answer to the request waiting on it, and reports when the host asks the
//! plugin to shut down or closes the connection.
//! A `Client` is cheap to clone, and every clone talks over the same
//! connection.
//!
//! Most requests are answered once ([`Client::request`]).
//! A `query` that starts a conversation is answered twice: with `created` as
//! soon as the conversation exists, and with `query_complete` when its first
//! turn ends ([`Client::send_expecting`]).
//!
//! The client sends [`PluginToHost`] messages and returns [`HostToPlugin`]
//! answers as the host sent them, error answers included: what a request means
//! is the plugin's business.
//! It logs to the host through the same connection ([`Log`]), what it can't use
//! of the host's messages included.

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

/// Where a plugin writes its messages to the host: its stdout, shared with
/// anything else that writes there, such as a log layer.
///
/// Each message is written whole while the lock is held, so two writers never
/// interleave their lines.
pub type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// The waiters for each request's answers, by request id, in the order the
/// answers are expected.
type Pending = Mutex<HashMap<String, VecDeque<oneshot::Sender<HostToPlugin>>>>;

/// A connection to the host, shared by every clone.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    writer: Writer,
    pending: Pending,
    next_id: AtomicU64,
}

/// An answer the host owes to a request, to wait on with [`Answer::wait`].
#[derive(Debug)]
pub struct Answer {
    rx: oneshot::Receiver<HostToPlugin>,
}

/// Why a request got no answer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The message couldn't be written to the host, which has most likely gone
    /// away.
    #[error("failed to write to the host: {0}")]
    Write(io::Error),

    /// The thread reading the host's messages couldn't be started.
    #[error("failed to start the host reader: {0}")]
    Listen(io::Error),

    /// The request carries no id, which is what its answer is matched by.
    #[error("the request carries no id to match its answer by")]
    NoId,

    /// The connection to the host closed before the answer came.
    #[error("the connection to the host closed")]
    Closed,

    /// No answer came within the timeout.
    #[error("timed out waiting for the host")]
    TimedOut,
}

impl Client {
    /// A client writing its requests to `writer`.
    ///
    /// It matches no answers until [`Client::listen`] starts reading them, so a
    /// plugin can answer the host's `init` through it first.
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

    /// Read the host's messages from `stdin` on a thread of its own, handing
    /// each answer to the request waiting on it.
    ///
    /// The returned receiver turns `true` when the host sends `shutdown`, and
    /// when it closes the connection, at which point every request still
    /// waiting fails with [`Error::Closed`].
    /// A line that isn't a protocol message, and an answer no request waits
    /// for, is logged and dropped.
    ///
    /// # Errors
    ///
    /// [`Error::Listen`] when the thread can't be started.
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

    /// The writer this client writes to the host through, for a log layer to
    /// write through too ([`LogHandle::activate`]).
    ///
    /// [`LogHandle::activate`]: crate::log::LogHandle::activate
    #[must_use]
    pub fn writer(&self) -> Writer {
        Arc::clone(&self.inner.writer)
    }

    /// A request id no other request of this client carries: `1`, `2`, and so
    /// on.
    #[must_use]
    pub fn next_id(&self) -> String {
        self.inner
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string()
    }

    /// Write `message` to the host as one line.
    ///
    /// # Errors
    ///
    /// [`Error::Write`] when the message can't be serialized or written.
    pub fn send(&self, message: &PluginToHost) -> Result<(), Error> {
        let json = serde_json::to_string(message).map_err(|error| Error::Write(error.into()))?;
        let mut writer = lock(&self.inner.writer);

        writeln!(writer, "{json}")
            .and_then(|()| writer.flush())
            .map_err(Error::Write)
    }

    /// Print `text` on the host's output channel `channel`: `content`, what a
    /// script reads on stdout, or `chrome`, notes on stderr that `--quiet`
    /// hides.
    ///
    /// # Errors
    ///
    /// The errors of [`Client::send`].
    pub fn print(&self, text: &str, channel: &str) -> Result<(), Error> {
        self.send(&PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: channel.to_owned(),
            format: "plain".into(),
            language: None,
        }))
    }

    /// Send `message` and wait for its answer: for up to `timeout`, or with no
    /// timeout for as long as the connection lasts.
    ///
    /// The request is forgotten when no answer comes.
    ///
    /// # Errors
    ///
    /// The errors of [`Client::send_expecting`] and [`Answer::wait`].
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

    /// Send `message`, which the host answers `N` times, and return the answers
    /// to wait on, in the order the host sends them.
    ///
    /// Each answer is waited for before the request goes out, so an answer the
    /// host sends at once can't arrive before anything waits for it.
    /// Nothing is left waiting when the request isn't sent.
    ///
    /// # Errors
    ///
    /// [`Error::NoId`] when `message` carries no request id, and the errors of
    /// [`Client::send`].
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

    /// Stop waiting for the answers still owed to the request `id`.
    ///
    /// Waiting on any of them then fails with [`Error::Closed`], and an answer
    /// the host sends for the request later is logged and dropped.
    pub fn forget(&self, id: &str) {
        lock(&self.inner.pending).remove(id);
    }

    /// Add a waiter for the next answer to the request `id`, behind any it has.
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
    /// Wait for the answer: for up to `timeout`, or with no timeout for as long
    /// as the connection lasts.
    ///
    /// # Errors
    ///
    /// [`Error::TimedOut`] when `timeout` passes first, and [`Error::Closed`]
    /// when the connection closes first or the request was forgotten.
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

/// Read the host's messages until it closes the connection, handing each answer
/// to the request waiting on it, and logging what can't be used.
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

    // The host is gone. Dropping every waiter fails each request still waiting
    // as closed, rather than leaving it to wait for an answer that can't come.
    lock(&client.inner.pending).clear();
    let _ = shutdown.send(true);
}

/// Hand `answer` to the first request waiting on its id, forgetting the request
/// once its last waiter is served.
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

    // A waiter that stopped waiting dropped its end, and the answer goes
    // nowhere.
    drop(waiter.send(answer));
}

/// The `type` a host message is tagged with on the wire, such as `done`.
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

/// Acquire `mutex`, recovering the guard if a holder panicked: the map of
/// waiters and the writer it guards stay usable whatever a holder was doing.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
