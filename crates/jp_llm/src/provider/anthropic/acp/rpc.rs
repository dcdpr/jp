//! JSON-RPC 2.0 over an agent's stdio, in the shape ACP uses it.
//!
//! [`drive`] owns one connection: it writes outgoing messages to the agent's
//! stdin, reads newline-delimited messages from its stdout, and runs a
//! foreground task that issues requests through a [`Peer`].
//!
//! Traffic in the other direction — the agent's notifications and its requests
//! to us — reaches the [`Handler`] this module is given.
//! A handler that fails ends the connection, which is how a rejected
//! notification aborts a request already in flight.
//!
//! The wire types live in `agent_client_protocol_schema`; this module only
//! frames them.
//! Method names come from that crate's `AGENT_METHOD_NAMES` and
//! `CLIENT_METHOD_NAMES` rather than string literals here, so a protocol rename
//! is a compile error instead of a silent no-op.

use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicI64, Ordering},
    },
};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader},
    sync::{mpsc, oneshot},
};
use tracing::debug;

/// A request JP sends to the agent.
///
/// The response type is part of the contract, so a caller cannot pair a request
/// with the wrong reply shape.
pub(super) trait Request: Serialize {
    /// The JSON-RPC method, taken from the schema crate's method-name table.
    const METHOD: &'static str;

    /// What the agent answers with.
    type Response: DeserializeOwned;
}

/// A JSON-RPC error, as both a received failure and one JP reports.
///
/// `data` carries whatever diagnostic context the sender attached; JP uses it
/// for the agent's exit status and its tail of stderr.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// The JSON-RPC code for a failure that is not the caller's fault.
    const INTERNAL: i64 = -32603;

    /// A rejection of the caller's arguments.
    ///
    /// JP only ever receives one of these; it is constructed here so a scripted
    /// adapter can send one.
    #[cfg(test)]
    pub(super) fn invalid_params() -> Self {
        Self {
            code: -32602,
            message: "Invalid params".into(),
            data: None,
        }
    }

    /// An internal error with no message beyond its code.
    pub(super) fn internal_error() -> Self {
        Self {
            code: Self::INTERNAL,
            message: "Internal error".into(),
            data: None,
        }
    }

    /// Report an arbitrary failure as an internal error, keeping its display
    /// text as the message.
    pub(super) fn into_internal_error(error: impl fmt::Display) -> Self {
        Self {
            code: Self::INTERNAL,
            message: error.to_string(),
            data: None,
        }
    }

    /// Attach diagnostic context, replacing whatever was there.
    #[must_use]
    pub(super) fn data(mut self, data: impl Into<Value>) -> Self {
        self.data = Some(data.into());
        self
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(data) = &self.data {
            write!(f, " ({data})")?;
        }
        Ok(())
    }
}

impl std::error::Error for RpcError {}

/// Something the agent sent that was not an answer to one of our requests.
pub(super) enum Inbound {
    /// A one-way message.
    /// Returning an error from its handler ends the connection.
    Notification { method: String, params: Value },

    /// A request awaiting our answer, which is the handler's returned value.
    Request { method: String, params: Value },
}

/// Handles everything the agent initiates.
///
/// Boxed rather than generic because one connection has exactly one handler and
/// it captures the whole translation state.
pub(super) type Handler =
    Box<dyn Fn(Inbound) -> BoxFuture<'static, Result<Value, RpcError>> + Send + Sync>;

/// Issues requests on an open connection.
///
/// Cloneable, so the foreground task and any handler can both use it.
#[derive(Clone)]
pub(super) struct Peer {
    outgoing: mpsc::UnboundedSender<Value>,
    pending: Pending,
    next_id: Arc<AtomicI64>,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Peer {
    /// Send a request and wait for its answer.
    ///
    /// Returns the agent's error when it rejects the request, and an internal
    /// error when the connection ends before the answer arrives.
    pub(super) async fn request<R: Request>(&self, params: R) -> Result<R::Response, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let params = serde_json::to_value(params).map_err(RpcError::into_internal_error)?;
        let (sender, receiver) = oneshot::channel();
        lock(&self.pending).insert(id, sender);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": R::METHOD, "params": params});
        if self.outgoing.send(message).is_err() {
            lock(&self.pending).remove(&id);
            return Err(RpcError::internal_error().data("ACP connection closed"));
        }
        let result = receiver
            .await
            .map_err(|_| RpcError::internal_error().data("ACP connection closed"))??;
        serde_json::from_value(result).map_err(RpcError::into_internal_error)
    }
}

/// Which end of a connection sent a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Side {
    /// JP, acting as the ACP client.
    Jp,

    /// The adapter JP is connected to.
    Agent,
}

/// Observes every framed message crossing a connection, in both directions.
///
/// The observer runs inline on the reader and writer tasks, so it must not
/// block: one that falls behind stalls the connection rather than the observer.
///
/// Where the messages go is [`super::cassette`]'s decision; this type is only
/// the place they pass through.
#[derive(Clone)]
pub(super) struct Tap(Option<Arc<Observer>>);

type Observer = dyn Fn(Side, &Value) + Send + Sync;

impl Tap {
    /// Observe nothing, at the cost of one null check per message.
    pub(super) fn none() -> Self {
        Self(None)
    }

    /// Observe every message with `observe`.
    pub(super) fn new(observe: impl Fn(Side, &Value) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(observe)))
    }

    /// Hand one message to the observer, if there is one.
    pub(super) fn observe(&self, side: Side, message: &Value) {
        if let Some(observer) = &self.0 {
            observer(side, message);
        }
    }
}

/// Run one connection until the foreground task finishes or the agent stops.
///
/// Returns the foreground task's result, unless the connection failed first: a
/// handler error, unreadable input, or the agent closing its output while a
/// request is still outstanding all end the connection with that failure.
pub(super) async fn drive<I, O, F, Fut>(
    input: I,
    output: O,
    tap: Tap,
    handler: Handler,
    foreground: F,
) -> Result<(), RpcError>
where
    I: AsyncWrite + Unpin + Send + 'static,
    O: AsyncRead + Unpin + Send + 'static,
    F: FnOnce(Peer) -> Fut,
    Fut: Future<Output = Result<(), RpcError>>,
{
    let (outgoing, mut queued) = mpsc::unbounded_channel();
    let pending: Pending = Pending::default();
    let peer = Peer {
        outgoing: outgoing.clone(),
        pending: pending.clone(),
        next_id: Arc::new(AtomicI64::new(1)),
    };

    let writer_tap = tap.clone();
    let mut writer = tokio::spawn(async move {
        let mut input = input;
        while let Some(message) = queued.recv().await {
            writer_tap.observe(Side::Jp, &message);
            let mut line = serde_json::to_vec(&message).map_err(RpcError::into_internal_error)?;
            line.push(b'\n');
            input
                .write_all(&line)
                .await
                .map_err(RpcError::into_internal_error)?;
            input.flush().await.map_err(RpcError::into_internal_error)?;
        }
        Ok::<(), RpcError>(())
    });

    let reader_outgoing = outgoing.clone();
    let mut reader = tokio::spawn(async move {
        let mut lines = BufReader::new(output).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(RpcError::into_internal_error)?
        {
            if line.trim().is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(&line) {
                Ok(message) => message,
                // A line that is not JSON at all is the agent talking past the
                // protocol; the connection survives it, since the request we
                // are waiting on may still be answered.
                Err(error) => {
                    debug!(%error, "Skipping unparseable ACP line");
                    continue;
                }
            };
            tap.observe(Side::Agent, &message);
            dispatch(message, &pending, &handler, &reader_outgoing).await?;
        }
        // The agent closed its output. Anything still waiting will never be
        // answered, so fail it rather than hang.
        for (_, sender) in lock(&pending).drain() {
            drop(sender.send(Err(
                RpcError::internal_error().data("ACP connection closed"),
            )));
        }
        Ok::<(), RpcError>(())
    });

    let outcome = tokio::select! {
        biased;
        reader = &mut reader => join(reader),
        writer = &mut writer => join(writer),
        result = foreground(peer) => result,
    };
    // Both tasks outlive the connection otherwise: the reader parks on the
    // agent's output and holds the handler, and through it whatever the handler
    // captured. A caller waiting for its own channel to close would wait on a
    // sender kept alive by a connection that has already finished.
    //
    // Anything still queued for the agent is dropped with the writer, which is
    // what a finished connection wants: the sequence is over either way.
    reader.abort();
    writer.abort();
    outcome
}

fn join(result: Result<Result<(), RpcError>, tokio::task::JoinError>) -> Result<(), RpcError> {
    result.map_err(RpcError::into_internal_error)?
}

/// Route one parsed message: an answer, a notification, or a request.
async fn dispatch(
    message: Value,
    pending: &Pending,
    handler: &Handler,
    outgoing: &mpsc::UnboundedSender<Value>,
) -> Result<(), RpcError> {
    let id = message.get("id").and_then(Value::as_i64);
    let method = message.get("method").and_then(Value::as_str);

    let Some(method) = method else {
        // No method means this answers one of our requests.
        let Some(id) = id else {
            debug!("Skipping ACP message with neither method nor id");
            return Ok(());
        };
        let Some(sender) = lock(pending).remove(&id) else {
            debug!(id, "Skipping ACP answer to an unknown request");
            return Ok(());
        };
        let answer = match message.get("error") {
            Some(error) => Err(serde_json::from_value(error.clone())
                .unwrap_or_else(|_| RpcError::internal_error().data(error.clone()))),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        drop(sender.send(answer));
        return Ok(());
    };

    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let method = method.to_owned();

    let Some(id) = id else {
        // A handler that refuses a notification ends the connection: the
        // authentication update uses this to stop a prompt already in flight.
        handler(Inbound::Notification { method, params }).await?;
        return Ok(());
    };

    let answer = handler(Inbound::Request { method, params }).await;
    let reply = match answer {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    };
    drop(outgoing.send(reply));
    Ok(())
}

#[cfg(test)]
#[path = "rpc_tests.rs"]
mod tests;
