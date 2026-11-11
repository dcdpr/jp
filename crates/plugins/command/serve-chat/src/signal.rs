//! Signal transport backed by a `signal-cli` child process in JSON-RPC mode.
//!
//! `signal-cli -a <account> jsonRpc` reads requests from stdin and writes
//! responses and inbound message notifications as JSON lines on stdout
//! (`signal-cli-jsonrpc(5)`).
//! The transport forwards incoming text messages to the bridge and sends
//! replies and typing indicators; attachments, avatars, stickers, and stories
//! are not downloaded.
//!
//! Outbound JSON-RPC requests are fire-and-forget: if a send fails, the error
//! response is logged when it arrives on stdout.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::{
    host::Level,
    options::SignalOptions,
    text::{chunks, styled},
    transport::{Chat, Inbound, Outbox},
};

/// Maximum UTF-8 bytes of styled text per outbound Signal message before
/// splitting.
///
/// Signal converts message bodies over 2,000 bytes into file attachments, which
/// some clients do not render inline.
const MAX_MESSAGE: usize = 1800;

pub(crate) type Logger = Arc<dyn Fn(Level, String) + Send + Sync>;

/// Handle to the `signal-cli` child process; kills and reaps the child on drop.
pub(crate) struct SignalProcess {
    child: Child,
}

impl Drop for SignalProcess {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// Outbox implementation that writes JSON-RPC requests to `signal-cli`'s stdin.
pub(crate) struct Signal {
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
    log: Logger,
}

impl Signal {
    /// Spawn `signal-cli` and background reader threads for its stdout and
    /// stderr.
    ///
    /// `inbound` closes when `signal-cli` exits, which stops the bridge.
    pub(crate) fn spawn(
        options: &SignalOptions,
        log: Logger,
        inbound: mpsc::Sender<Inbound>,
    ) -> Result<(Arc<Self>, SignalProcess), String> {
        let mut child = Command::new(&options.command)
            .args(["-a", &options.account, "jsonRpc"])
            .args([
                "--ignore-attachments",
                "--ignore-avatars",
                "--ignore-stickers",
                "--ignore-stories",
                "--send-read-receipts",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start `{}`: {error}", options.command))?;

        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let reader_log = Arc::clone(&log);
        let own_ids = options.own_ids();
        thread::Builder::new()
            .name("signal-reader".into())
            .spawn(move || {
                read_messages(BufReader::new(stdout), &own_ids, &inbound, &reader_log);
            })
            .map_err(|error| format!("failed to start the Signal reader: {error}"))?;

        let stderr_log = Arc::clone(&log);
        thread::Builder::new()
            .name("signal-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    stderr_log(Level::Info, format!("signal-cli: {line}"));
                }
            })
            .map_err(|error| format!("failed to start the Signal log reader: {error}"))?;

        let signal = Arc::new(Self {
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            log,
        });

        Ok((signal, SignalProcess { child }))
    }

    fn request(&self, method: &str, params: &Map<String, Value>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let line = request(id, method, params);

        let mut stdin = self.stdin.lock().expect("signal stdin lock poisoned");
        if let Err(error) = writeln!(stdin, "{line}").and_then(|()| stdin.flush()) {
            (self.log)(
                Level::Error,
                format!("failed to write to signal-cli: {error}"),
            );
        }
    }
}

impl Outbox for Signal {
    fn send(&self, chat: &Chat, markdown: &str) {
        for chunk in chunks(markdown, MAX_MESSAGE) {
            self.request("send", &send_params(chat, &chunk));
        }
    }

    fn typing(&self, chat: &Chat, on: bool) {
        let mut params = recipient(chat);
        if !on {
            params.insert("stop".to_owned(), Value::Bool(true));
        }
        self.request("sendTyping", &params);
    }
}

/// Format a JSON-RPC 2.0 request line.
pub(crate) fn request(id: u64, method: &str, params: &Map<String, Value>) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id.to_string(),
        "method": method,
        "params": params,
    })
    .to_string()
}

/// Build the recipient parameter (`recipient` or `groupId`) for `chat`.
fn recipient(chat: &Chat) -> Map<String, Value> {
    let mut params = Map::new();
    match chat {
        Chat::Direct(account) => {
            params.insert("recipient".to_owned(), json!([account]));
        }
        Chat::Group(group) => {
            params.insert("groupId".to_owned(), json!(group));
        }
    }

    params
}

/// Build `send` request parameters for `chat` with `markdown` converted to
/// Signal styles.
pub(crate) fn send_params(chat: &Chat, markdown: &str) -> Map<String, Value> {
    let styled = styled(markdown);
    let mut params = recipient(chat);
    params.insert("message".to_owned(), Value::String(styled.text));
    if !styled.styles.is_empty() {
        params.insert("textStyle".to_owned(), json!(styled.styles));
    }

    params
}

/// Parsed event from a single line of `signal-cli` stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Line {
    Message(Inbound),

    Failed(String),

    /// Non-message notification (receipt, typing indicator, sync event) or a
    /// successful RPC response.
    Ignored,
}

/// Parse one line of `signal-cli` stdout.
///
/// `own_ids` lists the bridge account's phone number and UUID so mentions of or
/// replies to the bridge set `Inbound::addressed = true`.
pub(crate) fn parse(line: &str, own_ids: &[String]) -> Line {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Line::Ignored;
    };

    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Line::Failed(message.to_owned());
    }

    if value.get("method").and_then(Value::as_str) != Some("receive") {
        return Line::Ignored;
    }
    let params = value.get("params");
    let envelope = params
        .and_then(|params| params.get("envelope"))
        .or_else(|| params.and_then(|params| params.pointer("/result/envelope")));

    envelope
        .and_then(|envelope| message(envelope, own_ids))
        .map_or(Line::Ignored, Line::Message)
}

/// Extract an [`Inbound`] message from a `signal-cli` `envelope`, if it
/// contains a `dataMessage` with text or attachments.
fn message(envelope: &Value, own_ids: &[String]) -> Option<Inbound> {
    let data = envelope.get("dataMessage")?;
    let raw = data
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mentions = data
        .get("mentions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let (text, mentioned) = resolve_mentions(raw, mentions, own_ids);
    let replied = data
        .get("quote")
        .is_some_and(|quote| is_own(quote, "authorNumber", "authorUuid", own_ids));
    let attachments = data
        .get("attachments")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if text.trim().is_empty() && attachments == 0 {
        return None;
    }

    let field = |name: &str| {
        envelope
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    let mut sender: Vec<String> = vec![];
    for id in ["sourceUuid", "sourceNumber", "source"]
        .into_iter()
        .filter_map(field)
    {
        if !sender.contains(&id) {
            sender.push(id);
        }
    }

    let group = data
        .pointer("/groupInfo/groupId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let chat = match group {
        Some(group) => Chat::Group(group),
        None => Chat::Direct(
            field("sourceUuid")
                .or_else(|| field("sourceNumber"))
                .or_else(|| field("source"))?,
        ),
    };

    Some(Inbound {
        chat,
        sender,
        name: field("sourceName"),
        addressed: mentioned || replied,
        text,
        attachments,
    })
}

/// Return whether the `number` or `uuid` field of `value` matches any ID in
/// `own_ids`.
fn is_own(value: &Value, number: &str, uuid: &str, own_ids: &[String]) -> bool {
    [number, uuid]
        .into_iter()
        .filter_map(|field| value.get(field).and_then(Value::as_str))
        .any(|id| own_ids.iter().any(|own| own == id))
}

/// Replace Signal mention placeholders in `text` and report whether any mention
/// targets `own_ids`.
///
/// Signal encodes each mention in the message body as a `U+FFFC` placeholder
/// accompanied by a `{start, length, number, uuid}` entry measured in UTF-16
/// code units.
/// Mentions of the bridge's own account are stripped (the caller marks the
/// message with `(to you)` via `Inbound::addressed`); mentions of other
/// accounts are rendered as `@<number>`, or `@someone` when only a UUID is
/// present.
fn resolve_mentions(text: &str, mentions: &[Value], own_ids: &[String]) -> (String, bool) {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    let mut spans: Vec<(usize, usize, &Value)> = mentions
        .iter()
        .filter_map(|mention| {
            let start = usize::try_from(mention.get("start")?.as_u64()?).ok()?;
            let length = usize::try_from(mention.get("length")?.as_u64()?).ok()?;
            Some((start, length, mention))
        })
        .collect();
    // Splice in reverse offset order so earlier UTF-16 indices remain valid.
    spans.sort_by_key(|(start, ..)| std::cmp::Reverse(*start));

    let mut mentioned = false;
    for (start, length, mention) in spans {
        let Some(end) = start.checked_add(length).filter(|end| *end <= units.len()) else {
            continue;
        };

        let replacement = if is_own(mention, "number", "uuid", own_ids) {
            mentioned = true;
            String::new()
        } else {
            match mention.get("number").and_then(Value::as_str) {
                Some(number) => format!("@{number}"),
                None => "@someone".to_owned(),
            }
        };
        units.splice(start..end, replacement.encode_utf16());
    }

    (String::from_utf16_lossy(&units), mentioned)
}

/// Read `signal-cli` stdout lines and forward messages to `inbound` until EOF.
fn read_messages(
    reader: impl BufRead,
    own_ids: &[String],
    inbound: &mpsc::Sender<Inbound>,
    log: &Logger,
) {
    for line in reader.lines().map_while(Result::ok) {
        match parse(&line, own_ids) {
            Line::Message(message) => {
                if inbound.blocking_send(message).is_err() {
                    return;
                }
            }
            Line::Failed(error) => log(Level::Warn, format!("signal-cli request failed: {error}")),
            Line::Ignored => {}
        }
    }

    log(Level::Error, "signal-cli exited.".to_owned());
}

#[cfg(test)]
#[path = "signal_tests.rs"]
mod tests;
