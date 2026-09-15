//! Recording and replay of one ACP conversation.
//!
//! A cassette is the stdio counterpart of `jp_test::mock::Vcr`, which records
//! HTTP by proxying it.
//! ACP speaks newline-delimited JSON-RPC over a child process's pipes, so there
//! is nothing for an HTTP proxy to intercept; what carries across is the
//! convention.
//! `RECORD` is the same switch, fixtures live under the same `tests/fixtures`
//! root, and playing back without a recording reports the path that is missing.
//!
//! A recording taken against the installed adapter is the only non-circular
//! evidence that [`super::schema`] spells the protocol's field names correctly.
//! Those types are written by hand, so a fixture written from the same reading
//! of the specification would agree with them whether or not the adapter does.
//!
//! One query can open several connections, and a recording holds all of them,
//! the way an HTTP cassette holds every exchange a test made.
//! Each line is one framed message, numbered by the connection it belongs to:
//! `{"connection": 0, "from": "jp"|"agent", "message": {...}}`.

use std::{
    collections::HashMap,
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use super::rpc::{Side, Tap};

/// One framed message, tagged with the connection and the end that sent it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Framed {
    /// Which connection carried it, counting from zero within one process.
    #[serde(default)]
    pub(super) connection: usize,

    /// Which end sent it.
    pub(super) from: Side,

    /// The JSON-RPC message itself, exactly as it crossed the pipe.
    pub(super) message: Value,
}

/// Where one recorded conversation lives.
///
/// Prefers the package's `tests/fixtures` directory, which is where the rest of
/// the workspace keeps its cassettes.
/// `CARGO_MANIFEST_DIR` is unset when a release binary records a real query,
/// and the working directory is the only location that binary can be said to
/// have.
fn fixture(name: &str) -> PathBuf {
    let root = env::var_os("CARGO_MANIFEST_DIR").map_or_else(
        || PathBuf::from("."),
        |root| PathBuf::from(root).join("tests/fixtures"),
    );

    root.join("acp").join(format!("{name}.jsonl"))
}

/// Whether this run records rather than replays.
///
/// Reads `RECORD`, the switch the workspace's HTTP cassettes already use, so
/// one habit covers both.
pub(super) fn recording() -> bool {
    env::var("RECORD").is_ok()
}

/// The recording each named fixture is accumulating in this process.
///
/// A query opens one connection per request, and all of them belong in one
/// file; truncating per connection would leave only the last.
static RECORDINGS: LazyLock<Mutex<HashMap<PathBuf, Arc<Recording>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One fixture being written, shared by every connection that lands in it.
struct Recording {
    file: Mutex<fs::File>,
    connections: AtomicUsize,
}

/// What a redacted value is replaced with.
const REDACTED: &str = "[redacted]";

/// Values a recording must not carry into the repository.
///
/// Each identifies the account that recorded it or the machine it ran on, and
/// none is read back: a replay answers by method rather than by arguments, and
/// JP supplies its own working directory either way.
///
/// Named fields rather than a shape, so a field the adapter adds later is
/// recorded as-is.
/// A recording only changes when somebody deliberately re-records it, and that
/// diff is read before it merges.
const SENSITIVE: &[&[&str]] = &[
    &["params", "authStatus", "account", "email"],
    &["params", "authStatus", "account", "organization"],
    &["params", "cwd"],
    &["params", "message", "cwd"],
];

/// Replace the value at `path`, if the message has one there.
fn redact(message: &mut Value, path: &[&str]) {
    let Some((leaf, parents)) = path.split_last() else {
        return;
    };

    let mut node = message;
    for key in parents {
        match node.get_mut(*key) {
            Some(next) => node = next,
            None => return,
        }
    }

    if let Some(value) = node.get_mut(*leaf) {
        *value = Value::String(REDACTED.to_owned());
    }
}

impl Recording {
    fn write(&self, connection: usize, from: Side, message: &Value) {
        let mut message = message.clone();
        for path in SENSITIVE {
            redact(&mut message, path);
        }

        let line = serde_json::to_string(&Framed {
            connection,
            from,
            message,
        })
        .expect("a framed message is JSON");
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = writeln!(file, "{line}") {
            warn!(%error, "Could not append to the ACP recording");
        }
    }
}

/// Observe a connection into `<name>.jsonl`, or observe nothing.
///
/// Returns an inert tap unless `RECORD` is set, so the production call site
/// carries no branch of its own.
/// An unwritable path disables recording rather than failing the query: a
/// qualification run that reaches the adapter and loses its transcript is still
/// a qualification run.
pub(super) fn tap(name: &str) -> Tap {
    if !recording() {
        return Tap::none();
    }

    recorder(&fixture(name))
}

/// Observe one connection into `path`, numbering it after its predecessors.
///
/// The file is truncated when this process first records into it, and appended
/// to by every connection after that.
fn recorder(path: &Path) -> Tap {
    let mut recordings = RECORDINGS.lock().unwrap_or_else(PoisonError::into_inner);
    let recording = match recordings.get(path) {
        Some(recording) => recording.clone(),
        None => match create(path) {
            Ok(file) => {
                info!(path = %path.display(), "Recording the ACP conversation");
                let recording = Arc::new(Recording {
                    file: Mutex::new(file),
                    connections: AtomicUsize::new(0),
                });
                recordings.insert(path.to_owned(), recording.clone());
                recording
            }
            Err(error) => {
                warn!(%error, path = %path.display(), "Could not open the ACP recording");
                return Tap::none();
            }
        },
    };
    drop(recordings);

    let connection = recording.connections.fetch_add(1, Ordering::Relaxed);
    Tap::new(move |from, message| recording.write(connection, from, message))
}

/// Truncate `path` and open it for writing, creating its directory.
fn create(path: &Path) -> std::io::Result<fs::File> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }

    fs::File::create(path)
}

/// Read `<name>.jsonl`, or explain which recording is missing.
///
/// # Errors
///
/// Returns an error when the file is absent, unreadable, or holds a line that
/// is not a framed message.
#[cfg(test)]
pub(super) fn read(name: &str) -> Result<Vec<Framed>, String> {
    let path = fixture(name);
    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("Recording not found at {}: {error}", path.display()))?;

    parse(&contents).map_err(|error| format!("{}: {error}", path.display()))
}

/// Split a recording into one script per connection, in connection order.
///
/// A query opens a connection per request, so a recording of one query holds
/// several.
/// Each script keeps its messages in the order they were recorded.
#[cfg(test)]
pub(super) fn connections(script: Vec<Framed>) -> Vec<Vec<Framed>> {
    let mut grouped: std::collections::BTreeMap<usize, Vec<Framed>> =
        std::collections::BTreeMap::new();
    for entry in script {
        grouped.entry(entry.connection).or_default().push(entry);
    }

    grouped.into_values().collect()
}

/// Parse a recording, one framed message per non-empty line.
///
/// # Errors
///
/// Returns the offending line number when a line is not a framed message.
#[cfg(test)]
fn parse(contents: &str) -> Result<Vec<Framed>, String> {
    contents
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line)
                .map_err(|error| format!("line {}: not a framed message: {error}", index + 1))
        })
        .collect()
}

/// Answers JP from a recording, over an in-memory pipe.
///
/// Consumes one recorded JP message for each message JP sends, checking that
/// their methods agree, then writes back every agent message the recording
/// places before JP's next one.
/// An answer to one of JP's requests carries the live request's id rather than
/// the recorded one, since a replayed run allocates its own.
///
/// A recording that stops agreeing with what JP sends ends the connection and
/// reports where, rather than letting JP wait for an answer that is never
/// coming.
#[cfg(test)]
pub(super) struct Recorded(pub(super) Vec<Framed>);

#[cfg(test)]
impl super::transport::Transport for Recorded {
    fn connect(
        self: Box<Self>,
        handler: super::rpc::Handler,
        foreground: super::transport::Foreground,
    ) -> futures::future::BoxFuture<'static, Result<(), super::rpc::RpcError>> {
        use futures::FutureExt as _;

        let (jp_writes, agent_reads) = tokio::io::duplex(1 << 16);
        let (agent_writes, jp_reads) = tokio::io::duplex(1 << 16);
        let divergence: Arc<Mutex<Option<String>>> = Arc::default();
        let reported = divergence.clone();

        tokio::spawn(async move {
            if let Err(reason) = serve(self.0, agent_reads, agent_writes).await {
                *divergence.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason);
            }
        });

        super::rpc::drive(jp_writes, jp_reads, Tap::none(), handler, foreground)
            .map(move |result| {
                // The divergence is the cause; whatever JP reported is the
                // symptom of a pipe that closed underneath it.
                match reported
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                {
                    Some(reason) => Err(super::rpc::RpcError::into_internal_error(reason)),
                    None => result,
                }
            })
            .boxed()
    }
}

/// Answer JP from the recording until it stops sending or the script runs out.
///
/// # Errors
///
/// Returns the point at which the recording stopped describing what JP sent.
#[cfg(test)]
async fn serve<R, W>(script: Vec<Framed>, reads: R, mut writes: W) -> Result<(), String>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncBufReadExt as _;

    let mut lines = tokio::io::BufReader::new(reads).lines();
    let mut cursor = flush(&script, 0, &HashMap::new(), &mut writes).await?;
    let mut ids: HashMap<i64, Value> = HashMap::new();

    while let Ok(Some(line)) = lines.next_line().await {
        let live: Value = serde_json::from_str(&line)
            .map_err(|error| format!("JP sent a line that is not JSON: {error}"))?;

        let Some(recorded) = script.get(cursor) else {
            return Err(format!(
                "JP sent {} after the recording ended",
                describe(&live)
            ));
        };
        if recorded.from != Side::Jp {
            return Err(format!(
                "the recording expects the agent to speak next, but JP sent {}",
                describe(&live)
            ));
        }
        if method(&recorded.message) != method(&live) {
            return Err(format!(
                "the recording has {} where JP sent {}",
                describe(&recorded.message),
                describe(&live)
            ));
        }
        if let (Some(recorded), Some(live)) = (
            recorded.message.get("id").and_then(Value::as_i64),
            live.get("id").cloned(),
        ) {
            ids.insert(recorded, live);
        }

        cursor = flush(&script, cursor + 1, &ids, &mut writes).await?;
    }

    Ok(())
}

/// Write every agent message from `cursor` up to JP's next one.
#[cfg(test)]
async fn flush<W: tokio::io::AsyncWrite + Unpin>(
    script: &[Framed],
    mut cursor: usize,
    ids: &HashMap<i64, Value>,
    writes: &mut W,
) -> Result<usize, String> {
    use tokio::io::AsyncWriteExt as _;

    while let Some(entry) = script.get(cursor).filter(|entry| entry.from == Side::Agent) {
        let mut message = entry.message.clone();
        // An answer to one of JP's requests: no method, and an id JP chose.
        if method(&message).is_none()
            && let Some(recorded) = message.get("id").and_then(Value::as_i64)
            && let Some(live) = ids.get(&recorded)
        {
            message["id"] = live.clone();
        }

        let mut line = serde_json::to_vec(&message).expect("a recorded message is JSON");
        line.push(b'\n');
        writes
            .write_all(&line)
            .await
            .map_err(|error| format!("could not replay {}: {error}", describe(&message)))?;
        // The caller is waiting on this message and will send nothing more
        // until it arrives, so a buffered writer has to be emptied rather than
        // left to fill. Neither writer used today buffers.
        writes
            .flush()
            .await
            .map_err(|error| format!("could not replay {}: {error}", describe(&message)))?;
        cursor += 1;
    }

    Ok(cursor)
}

/// The JSON-RPC method, when a message has one.
#[cfg(test)]
fn method(message: &Value) -> Option<&str> {
    message.get("method").and_then(Value::as_str)
}

/// Name a message the way a divergence report should: by method, or by the
/// request it answers.
#[cfg(test)]
fn describe(message: &Value) -> String {
    match method(message) {
        Some(method) => format!("`{method}`"),
        None => match message.get("id") {
            Some(id) => format!("an answer to request {id}"),
            None => "a message with neither method nor id".to_owned(),
        },
    }
}

#[cfg(test)]
#[path = "cassette_tests.rs"]
mod tests;
