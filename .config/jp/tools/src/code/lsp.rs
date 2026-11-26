//! Blocking Language Server Protocol client for single-call workspace queries.
//!
//! [`Server::start`] spawns the language server and completes the `initialize`
//! handshake, [`Server::wait_until_ready`] blocks until the workspace is
//! indexed, and [`Server::request`] sends a request and waits for its response.
//! Dropping a [`Server`] terminates the child process if [`Server::shutdown`]
//! did not complete.
//!
//! JSON-RPC framing and message routing live in [`Connection`], which operates
//! over arbitrary readers and writers so it can be unit-tested without a child
//! process.

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    io::{self, BufRead, BufReader, Write},
    process::{Child, ChildStderr, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use url::Url;

/// JSON-RPC error code returned when a request is canceled because workspace
/// contents changed during evaluation.
/// Retrying the request resolves the error.
const CONTENT_MODIFIED: i64 = -32801;

/// Maximum retry attempts for requests that fail with [`CONTENT_MODIFIED`].
const CONTENT_MODIFIED_RETRIES: usize = 3;

/// Maximum lines of server stderr retained for error diagnostics.
const STDERR_TAIL_LINES: usize = 20;

/// Maximum time [`Server::shutdown`] waits for the child process to exit.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// Errors returned by language server operations.
#[derive(Debug)]
pub enum LspError {
    /// The server process could not be started.
    Spawn { command: String, error: io::Error },

    /// The server answered a request with an error.
    Response {
        method: String,
        code: i64,
        message: String,
    },

    /// The server did not answer in time.
    Timeout { method: String, after: Duration },

    /// The server closed the connection, or exited.
    Closed { detail: String },

    /// The server reported that it could not load the workspace.
    Unhealthy { message: String },

    /// Writing a message failed, or a response could not be deserialized.
    Protocol { method: String, message: String },
}

impl fmt::Display for LspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { command, error } => write!(f, "Could not start `{command}`: {error}"),
            Self::Response {
                method,
                code,
                message,
            } => write!(f, "`{method}` failed ({code}): {message}"),
            Self::Timeout { method, after } => {
                write!(f, "`{method}` got no answer within {}s", after.as_secs())
            }
            Self::Closed { detail } => write!(f, "The language server stopped: {detail}"),
            Self::Unhealthy { message } => {
                write!(
                    f,
                    "The language server could not load the workspace: {message}"
                )
            }
            Self::Protocol { method, message } => write!(f, "`{method}`: {message}"),
        }
    }
}

impl std::error::Error for LspError {}

/// Character offset encoding negotiated with the language server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
}

impl PositionEncoding {
    /// Convert byte offset `byte` in `line` to a character offset in this
    /// encoding.
    ///
    /// `byte` must lie on a UTF-8 character boundary.
    #[must_use]
    pub fn character(self, line: &str, byte: usize) -> u32 {
        let prefix = &line[..byte.min(line.len())];
        let units = match self {
            Self::Utf8 => prefix.len(),
            Self::Utf16 => prefix.chars().map(char::len_utf16).sum(),
        };

        u32::try_from(units).unwrap_or(u32::MAX)
    }

    /// Convert a `character` offset in `line` to a byte offset, returning
    /// `None` if `character` is past the end of `line` or inside a multi-unit
    /// character.
    #[must_use]
    pub fn byte(self, line: &str, character: u32) -> Option<usize> {
        let character = usize::try_from(character).ok()?;
        let mut units = 0;
        for (at, c) in line.char_indices() {
            if units == character {
                return Some(at);
            }
            units += match self {
                Self::Utf8 => c.len_utf8(),
                Self::Utf16 => c.len_utf16(),
            };
            if units > character {
                return None;
            }
        }

        (units == character).then_some(line.len())
    }
}

/// Server-specific notification used to detect when workspace indexing
/// finishes.
#[derive(Debug, Clone, Copy)]
pub struct ReadySignal {
    /// Notification method name carrying progress updates.
    pub method: &'static str,

    /// Inspects notification parameters and returns `None` while still loading,
    /// `Some(Ok(()))` when ready, or `Some(Err(message))` if workspace loading
    /// failed.
    pub ready: fn(&Value) -> Option<Result<(), String>>,
}

/// Inbound message from the server after server-to-client requests have been
/// handled.
#[derive(Debug)]
enum Incoming {
    Response {
        id: i64,
        outcome: Result<Value, (i64, String)>,
    },
    Notification {
        method: String,
        params: Value,
    },
}

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// A JSON-RPC connection to a language server.
///
/// A background thread reads messages, answers every server-to-client request
/// with a `null` result, and forwards responses and notifications to the
/// caller.
pub struct Connection {
    writer: SharedWriter,
    incoming: Receiver<Result<Incoming, String>>,
    next_id: i64,

    /// Most recent parameters received for each notification method.
    latest: HashMap<String, Value>,

    closed: Option<String>,
}

impl Connection {
    pub fn new(reader: impl BufRead + Send + 'static, writer: impl Write + Send + 'static) -> Self {
        let writer: SharedWriter = Arc::new(Mutex::new(Box::new(writer)));
        let (sender, incoming) = unbounded();
        let replies = Arc::clone(&writer);
        thread::spawn(move || read_loop(reader, &replies, &sender));

        Self {
            writer,
            incoming,
            next_id: 1,
            latest: HashMap::new(),
            closed: None,
        }
    }

    /// Send a request and wait up to `timeout` for its result.
    pub fn request(
        &mut self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(
            method,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )?;

        let deadline = Instant::now() + timeout;
        loop {
            if let Incoming::Response {
                id: answered,
                outcome,
            } = self.receive(method, deadline, timeout)?
                && answered == id
            {
                return outcome.map_err(|(code, message)| LspError::Response {
                    method: method.to_owned(),
                    code,
                    message,
                });
            }
        }
    }

    /// Send a notification.
    pub fn notify(&mut self, method: &str, params: &Value) -> Result<(), LspError> {
        self.send(
            method,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
    }

    /// Block until `signal` reports that the server has finished loading the
    /// workspace.
    ///
    /// Returns [`LspError::Unhealthy`] if the server finishes in an error
    /// state.
    pub fn wait_until_ready(
        &mut self,
        signal: ReadySignal,
        timeout: Duration,
    ) -> Result<(), LspError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(outcome) = self.latest.get(signal.method).and_then(signal.ready) {
                return outcome.map_err(|message| LspError::Unhealthy { message });
            }

            self.receive(signal.method, deadline, timeout)?;
        }
    }

    /// Receive the next message from the server, recording any notification in
    /// `self.latest`.
    fn receive(
        &mut self,
        method: &str,
        deadline: Instant,
        timeout: Duration,
    ) -> Result<Incoming, LspError> {
        if let Some(detail) = &self.closed {
            return Err(LspError::Closed {
                detail: detail.clone(),
            });
        }

        let wait = deadline.saturating_duration_since(Instant::now());
        let message = match self.incoming.recv_timeout(wait) {
            Ok(Ok(message)) => message,
            Ok(Err(detail)) => {
                self.closed = Some(detail.clone());
                return Err(LspError::Closed { detail });
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(LspError::Timeout {
                    method: method.to_owned(),
                    after: timeout,
                });
            }
            Err(RecvTimeoutError::Disconnected) => {
                let detail = "the connection closed".to_owned();
                self.closed = Some(detail.clone());
                return Err(LspError::Closed { detail });
            }
        };

        if let Incoming::Notification { method, params } = &message {
            self.latest.insert(method.clone(), params.clone());
        }

        Ok(message)
    }

    fn send(&self, method: &str, message: &Value) -> Result<(), LspError> {
        let mut writer = self.writer.lock().map_err(|_| LspError::Protocol {
            method: method.to_owned(),
            message: "the writer lock is poisoned".to_owned(),
        })?;

        write_message(&mut **writer, message).map_err(|error| LspError::Closed {
            detail: format!("sending `{method}` failed: {error}"),
        })
    }
}

/// Read messages from `reader` until EOF, replying to server-to-client requests
/// with `null` and forwarding responses and notifications to `sender`.
///
/// Sends a final `Err` describing why the stream ended before returning.
fn read_loop(
    mut reader: impl BufRead,
    writer: &SharedWriter,
    sender: &Sender<Result<Incoming, String>>,
) {
    let detail = loop {
        let message = match read_message(&mut reader) {
            Ok(Some(message)) => message,
            Ok(None) => break "the server closed its output".to_owned(),
            Err(error) => break format!("reading from the server failed: {error}"),
        };

        let incoming = match route(message) {
            Route::Forward(incoming) => incoming,
            Route::Answer(id) => {
                // Reply `null` to every server-to-client request. We advertise
                // no client capabilities that require a non-null response, and
                // leaving a request unanswered can stall the server.
                let reply = json!({ "jsonrpc": "2.0", "id": id, "result": null });
                if let Ok(mut writer) = writer.lock() {
                    let _written = write_message(&mut **writer, &reply);
                }
                continue;
            }
            Route::Ignore => continue,
        };

        if sender.send(Ok(incoming)).is_err() {
            return;
        }
    };

    let _sent = sender.send(Err(detail));
}

enum Route {
    Forward(Incoming),
    Answer(Value),
    Ignore,
}

/// Classify a decoded message as a response, a notification, or a request from
/// the server.
fn route(mut message: Value) -> Route {
    let id = message.get_mut("id").map(Value::take);
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);

    match (method, id) {
        (Some(_), Some(id)) => Route::Answer(id),
        (Some(method), None) => Route::Forward(Incoming::Notification {
            method,
            params: message.get_mut("params").map_or_default(Value::take),
        }),
        (None, Some(Value::Number(id))) => {
            let Some(id) = id.as_i64() else {
                return Route::Ignore;
            };

            let outcome = match message.get("error") {
                Some(error) => Err((
                    error
                        .get("code")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                )),
                None => Ok(message.get_mut("result").map_or_default(Value::take)),
            };

            Route::Forward(Incoming::Response { id, outcome })
        }
        _ => Route::Ignore,
    }
}

/// Read one `Content-Length`-framed JSON-RPC message, returning `Ok(None)` on
/// clean EOF between messages.
fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut started = false;
    let mut header = String::new();

    loop {
        header.clear();
        if reader.read_line(&mut header)? == 0 {
            if started {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the stream ended inside a message header",
                ));
            }
            return Ok(None);
        }

        let line = header.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if started {
                break;
            }
            continue;
        }

        started = true;
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
        }
    }

    let length = length.ok_or_else(|| io::Error::other("a message has no Content-Length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;

    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

fn write_message(writer: &mut dyn Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// A running language server process.
pub struct Server {
    child: Child,
    connection: Connection,
    stderr: Arc<Mutex<VecDeque<String>>>,
    encoding: PositionEncoding,
    timeout: Duration,
}

impl Server {
    /// Start `command` in `root` and complete the `initialize` handshake.
    ///
    /// `env` is added to the process's environment.
    /// `options` is sent as `initializationOptions`, and `experimental` as the
    /// client's experimental capabilities.
    /// `timeout` bounds each request, the handshake included.
    pub fn start(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        root: &Utf8Path,
        options: &Value,
        experimental: &Value,
        timeout: Duration,
    ) -> Result<Self, LspError> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env.iter().map(|(key, value)| (key, value)))
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| LspError::Spawn {
                command: command.to_owned(),
                error,
            })?;

        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(LspError::Closed {
                detail: "the server's standard streams were not captured".to_owned(),
            });
        };

        let mut server = Self {
            child,
            connection: Connection::new(BufReader::new(stdout), stdin),
            stderr: collect_stderr(stderr),
            encoding: PositionEncoding::Utf16,
            timeout,
        };

        let initialized = server.request_value(
            "initialize",
            &initialize_params(root, options, experimental),
        )?;
        if initialized
            .pointer("/capabilities/positionEncoding")
            .and_then(Value::as_str)
            == Some("utf-8")
        {
            server.encoding = PositionEncoding::Utf8;
        }

        server
            .connection
            .notify("initialized", &json!({}))
            .map_err(|error| server.explain(error))?;

        Ok(server)
    }

    #[must_use]
    pub fn encoding(&self) -> PositionEncoding {
        self.encoding
    }

    pub fn notify(&mut self, method: &str, params: &Value) -> Result<(), LspError> {
        self.connection
            .notify(method, params)
            .map_err(|error| self.explain(error))
    }

    /// Block until the server has loaded the workspace, for at most `timeout`.
    pub fn wait_until_ready(
        &mut self,
        signal: ReadySignal,
        timeout: Duration,
    ) -> Result<(), LspError> {
        self.connection
            .wait_until_ready(signal, timeout)
            .map_err(|error| self.explain(error))
    }

    /// Send a request and deserialize its response, retrying on
    /// [`CONTENT_MODIFIED`].
    pub fn request<R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<R, LspError> {
        let value = self.request_value(method, params)?;

        serde_json::from_value(value).map_err(|error| LspError::Protocol {
            method: method.to_owned(),
            message: format!("unexpected result: {error}"),
        })
    }

    fn request_value(&mut self, method: &str, params: &Value) -> Result<Value, LspError> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            match self.connection.request(method, params, self.timeout) {
                Err(LspError::Response { code, .. })
                    if code == CONTENT_MODIFIED && attempts <= CONTENT_MODIFIED_RETRIES => {}
                result => return result.map_err(|error| self.explain(error)),
            }
        }
    }

    /// Send `shutdown` and `exit` and wait up to [`EXIT_GRACE`] for the child
    /// process to exit.
    ///
    /// Ignores errors because [`Drop`] kills the process if it is still
    /// running.
    pub fn shutdown(&mut self) {
        if self
            .connection
            .request("shutdown", &Value::Null, EXIT_GRACE)
            .is_ok()
        {
            let _sent = self.connection.notify("exit", &Value::Null);
        }

        let deadline = Instant::now() + EXIT_GRACE;
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Append captured stderr lines to [`LspError::Closed`] errors.
    fn explain(&self, error: LspError) -> LspError {
        let LspError::Closed { detail } = error else {
            return error;
        };

        let tail = self
            .stderr
            .lock()
            .map_or_default(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n"));

        if tail.is_empty() {
            return LspError::Closed { detail };
        }

        LspError::Closed {
            detail: format!("{detail}\n\nThe server's last output:\n{tail}"),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _killed = self.child.kill();
        }
        let _reaped = self.child.wait();
    }
}

/// Continuously drain `stderr` on a background thread, retaining the last
/// [`STDERR_TAIL_LINES`] lines so the child process never blocks on a full
/// pipe.
fn collect_stderr(stderr: ChildStderr) -> Arc<Mutex<VecDeque<String>>> {
    let lines = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
    let tail = Arc::clone(&lines);

    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { return };
            let Ok(mut tail) = tail.lock() else { return };
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    });

    lines
}

fn initialize_params(root: &Utf8Path, options: &Value, experimental: &Value) -> Value {
    let uri = Url::from_directory_path(root.as_std_path()).map_or_default(String::from);

    json!({
        "processId": std::process::id(),
        "rootUri": uri,
        "workspaceFolders": [{ "uri": uri, "name": root.file_name().unwrap_or("workspace") }],
        "initializationOptions": options,
        "capabilities": {
            "general": { "positionEncodings": ["utf-8", "utf-16"] },
            "textDocument": {
                "synchronization": {},
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "definition": { "linkSupport": false },
                "implementation": { "linkSupport": false },
                "references": {},
                "selectionRange": {},
                "callHierarchy": {},
                "rename": {},
            },
            "workspace": { "symbol": {} },
            "experimental": experimental,
        },
    })
}

#[cfg(test)]
#[path = "lsp_tests.rs"]
mod tests;
