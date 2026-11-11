//! Signal transport backed by a `signal-cli` child process in JSON-RPC mode.
//!
//! `signal-cli -a <account> jsonRpc` reads requests from stdin and writes
//! responses and inbound message notifications as JSON lines on stdout
//! (`signal-cli-jsonrpc(5)`).
//! The transport forwards incoming messages to the bridge and sends replies and
//! typing indicators.
//! `signal-cli` saves received attachments in its attachments directory, and a
//! message carries their paths; avatars, stickers, and stories are not
//! downloaded.
//!
//! Outbound JSON-RPC requests are fire-and-forget: if a send fails, the error
//! response is logged when it arrives on stdout.

use std::{
    env,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use camino::Utf8PathBuf;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::{
    host::Level,
    lock,
    options::SignalOptions,
    text::{chunks, styled},
    transport::{Attachment, Chat, Inbound, Outbox},
};

/// Maximum UTF-8 bytes of styled text per outbound Signal message before
/// splitting.
///
/// Signal converts message bodies over 2,000 bytes into file attachments, which
/// some clients do not render inline.
const MAX_MESSAGE: usize = 1800;

pub(crate) type Logger = Arc<dyn Fn(Level, String) + Send + Sync>;

/// The bridge's own Signal account, as reading its messages needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    /// The account's phone number and UUID, which mark a mention of the bridge
    /// or a reply to it.
    pub ids: Vec<String>,

    /// Where `signal-cli` saves the attachments it receives.
    pub attachments: Utf8PathBuf,
}

impl Account {
    fn new(options: &SignalOptions) -> Result<Self, String> {
        let attachments = options
            .attachments_dir
            .clone()
            .or_else(default_attachments_dir)
            .ok_or(
                "Neither `XDG_DATA_HOME` nor `HOME` is set, so where `signal-cli` saves \
                 attachments is unknown; set `signal.attachments_dir`.",
            )?;

        Ok(Self {
            ids: options.own_ids(),
            attachments,
        })
    }
}

/// `signal-cli`'s default attachments directory, under its data directory.
fn default_attachments_dir() -> Option<Utf8PathBuf> {
    let non_empty = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
    let data = non_empty("XDG_DATA_HOME")
        .map(Utf8PathBuf::from)
        .or_else(|| non_empty("HOME").map(|home| Utf8PathBuf::from(home).join(".local/share")))?;

    Some(data.join("signal-cli/attachments"))
}

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
        let account = Account::new(options)?;
        let mut child = Command::new(&options.command)
            .args(["-a", &options.account, "jsonRpc"])
            .args([
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

        // Piped above, so each is there; a `None` would be a bug in that setup.
        let missing = |pipe: &str| format!("`{}` started without its {pipe}", options.command);
        let stdin = child.stdin.take().ok_or_else(|| missing("stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| missing("stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;

        let reader_log = Arc::clone(&log);
        thread::Builder::new()
            .name("signal-reader".into())
            .spawn(move || {
                read_messages(BufReader::new(stdout), &account, &inbound, &reader_log);
            })
            .map_err(|error| format!("failed to start the Signal reader: {error}"))?;

        let stderr_log = Arc::clone(&log);
        thread::Builder::new()
            .name("signal-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    stderr_log(Level::INFO, format!("signal-cli: {line}"));
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

        let mut stdin = lock(&self.stdin);
        if let Err(error) = writeln!(stdin, "{line}").and_then(|()| stdin.flush()) {
            (self.log)(
                Level::ERROR,
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

/// Parse one line of `signal-cli` stdout, received by `account`.
pub(crate) fn parse(line: &str, account: &Account) -> Line {
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
        .and_then(|envelope| message(envelope, account))
        .map_or(Line::Ignored, Line::Message)
}

/// Extract an [`Inbound`] message from a `signal-cli` `envelope`, if it
/// contains a `dataMessage` with text or attachments.
fn message(envelope: &Value, account: &Account) -> Option<Inbound> {
    let own_ids = &account.ids;
    let data = envelope.get("dataMessage")?;
    let raw = data
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mentions = data
        .get("mentions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let (text, mentioned, leading_mention) = resolve_mentions(raw, mentions, own_ids);
    let replied = data
        .get("quote")
        .is_some_and(|quote| is_own(quote, "authorNumber", "authorUuid", own_ids));
    let attachments = attachments(data, account);
    if text.trim().is_empty() && attachments.is_empty() {
        return None;
    }

    let sender = sender(envelope);
    let group = data
        .pointer("/groupInfo/groupId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let chat = match group {
        Some(group) => Chat::Group(group),
        None => Chat::Direct(sender.first()?.clone()),
    };

    Some(Inbound {
        chat,
        sender,
        name: field(envelope, "sourceName"),
        addressed: mentioned || replied,
        leading_mention,
        text,
        attachments,
    })
}

/// The attachments of a `dataMessage`, with where `signal-cli` saved each.
///
/// `signal-cli` names the saved file by the attachment's `id`, using only
/// letters, digits, `_`, `.`, and `-`; an `id` holding anything else, or none,
/// gets no path rather than one that could point outside the directory.
fn attachments(data: &Value, account: &Account) -> Vec<Attachment> {
    let Some(attachments) = data.get("attachments").and_then(Value::as_array) else {
        return vec![];
    };

    attachments
        .iter()
        .map(|attachment| Attachment {
            content_type: field(attachment, "contentType")
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            name: field(attachment, "filename"),
            path: field(attachment, "id")
                .filter(|id| is_file_name(id))
                .map(|id| account.attachments.join(id)),
        })
        .collect()
}

/// Whether `id` is a plain file name in `signal-cli`'s spelling.
fn is_file_name(id: &str) -> bool {
    !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The non-empty string at `name` in `envelope`.
fn field(envelope: &Value, name: &str) -> Option<String> {
    envelope
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// The sender's identifiers in `envelope`, most stable first and without
/// repeats: the account UUID, then the phone number, which a sender may hide.
fn sender(envelope: &Value) -> Vec<String> {
    let mut ids: Vec<String> = vec![];
    for id in ["sourceUuid", "sourceNumber", "source"]
        .into_iter()
        .filter_map(|name| field(envelope, name))
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }

    ids
}

/// Return whether the `number` or `uuid` field of `value` matches any ID in
/// `own_ids`.
fn is_own(value: &Value, number: &str, uuid: &str, own_ids: &[String]) -> bool {
    [number, uuid]
        .into_iter()
        .filter_map(|field| value.get(field).and_then(Value::as_str))
        .any(|id| own_ids.iter().any(|own| own == id))
}

/// Replace Signal mention placeholders in `text`, and report whether any
/// mention targets `own_ids`, and whether the text starts with one that does.
///
/// Signal encodes each mention in the message body as a `U+FFFC` placeholder
/// accompanied by a `{start, length, number, uuid}` entry measured in UTF-16
/// code units.
/// Mentions of the bridge's own account are stripped (the caller marks the
/// message with `(to you)` via `Inbound::addressed`); mentions of other
/// accounts are rendered as `@<number>`, or `@someone` when only a UUID is
/// present.
fn resolve_mentions(text: &str, mentions: &[Value], own_ids: &[String]) -> (String, bool, bool) {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    // Before any splice: the start offsets index the original text.
    let leading = units
        .iter()
        .position(|unit| !char::from_u32(u32::from(*unit)).is_some_and(char::is_whitespace));
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
    let mut leading_mention = false;
    for (start, length, mention) in spans {
        let Some(end) = start.checked_add(length).filter(|end| *end <= units.len()) else {
            continue;
        };

        let replacement = if is_own(mention, "number", "uuid", own_ids) {
            mentioned = true;
            leading_mention |= Some(start) == leading;
            String::new()
        } else {
            match mention.get("number").and_then(Value::as_str) {
                Some(number) => format!("@{number}"),
                None => "@someone".to_owned(),
            }
        };
        units.splice(start..end, replacement.encode_utf16());
    }

    (String::from_utf16_lossy(&units), mentioned, leading_mention)
}

/// Read `signal-cli` stdout lines and forward messages to `inbound` until EOF.
fn read_messages(
    reader: impl BufRead,
    account: &Account,
    inbound: &mpsc::Sender<Inbound>,
    log: &Logger,
) {
    for line in reader.lines().map_while(Result::ok) {
        match parse(&line, account) {
            Line::Message(message) => {
                if inbound.blocking_send(message).is_err() {
                    return;
                }
            }
            Line::Failed(error) => log(Level::WARN, format!("signal-cli request failed: {error}")),
            Line::Ignored => {}
        }
    }

    log(Level::ERROR, "signal-cli exited.".to_owned());
}

#[cfg(test)]
#[path = "signal_tests.rs"]
mod tests;
