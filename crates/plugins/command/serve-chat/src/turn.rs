//! Building a turn's request and running it on the `jp` host.
//!
//! [`run_turn`] reads the configured tools, prepends the override that disables
//! all of them ([`turn_cfg`]), and then starts the turn on the chat's
//! conversation, or in a new one.
//! [`cfg`] is the persona and sender attribution every turn runs under.

use chrono::NaiveDate;
use serde_json::{Map, Value, json};
use tokio::{sync::mpsc, time::Instant};

use crate::{
    bridge::Event,
    host::{Host, NewTurn},
    options::{Options, Rollover},
    queue::Waiting,
    transport::Chat,
};

/// Build the `--cfg` arguments for a turn triggered by `person`.
///
/// Applies the configured persona, sets `user.name` and the plugin `assistant`
/// attribution to `<Name> via jp`, and forces `conversation.start_local = true`
/// so conversation files are stored outside the workspace checkout.
pub(crate) fn cfg(options: &Options, person: &str) -> Vec<String> {
    let name = options
        .people
        .get(person)
        .map_or(person, |person| person.name.as_str());
    let author = format!("{name} via jp");

    let mut cfg: Vec<String> = options.persona.iter().cloned().collect();
    cfg.extend(options.cfg.iter().cloned());
    // Pass each plugin option as a dotted key=value assignment so merging
    // preserves the plugin's `run` policy from the workspace config.
    cfg.extend([
        format!("user.name={name}"),
        "conversation.start_local=true".to_owned(),
        format!("plugins.command.ticket.options.assistant={author}"),
        format!("plugins.command.rfd.options.assistant={author}"),
        format!("plugins.command.wiki.options.assistant={author}"),
    ]);

    cfg
}

/// Format the conversation title for `waiting` in the rollover period starting
/// on `period`.
pub(crate) fn title(
    options: &Options,
    rollover: Rollover,
    waiting: &Waiting,
    period: NaiveDate,
) -> String {
    let when = match rollover {
        Rollover::Daily => period.to_string(),
        Rollover::Weekly(_) => format!("week of {period}"),
    };

    match waiting.chat {
        Chat::Direct(_) => {
            let name = options
                .people
                .get(&waiting.person)
                .map_or(waiting.person.as_str(), |person| person.name.as_str());
            format!("Chat with {name}, {when}")
        }
        Chat::Group(_) => format!("Group chat, {when}"),
    }
}

/// Parameters for running a turn on the host.
pub(crate) struct TurnRequest {
    pub content: String,
    pub title: String,

    /// [`cfg()`], before [`turn_cfg`] prepends the tool-disabling override.
    pub cfg: Vec<String>,

    pub expires_in: String,
}

/// Run a turn on `conversation` (or create a new conversation when `None`) and
/// await its completion.
///
/// Emits [`Event::Started`] once the host accepts the request.
/// Fails before starting the turn if the configured tool list cannot be read,
/// so a turn never runs without first disabling unapproved tools.
pub(crate) async fn run_turn<H: Host>(
    host: &H,
    conversation: Option<String>,
    request: TurnRequest,
    events: &mpsc::UnboundedSender<Event>,
) -> Result<(), String> {
    let asked = Instant::now();
    let tools = host
        .tools()
        .await
        .map_err(|error| format!("couldn't read the configured tools: {error}"))?;
    let reading_tools = asked.elapsed();
    let cfg = turn_cfg(&tools, request.cfg)?;

    let (conversation, turn) = match conversation {
        Some(conversation) => {
            let turn = host.query(&conversation, &request.content, cfg);
            (conversation, turn)
        }
        None => {
            host.start(NewTurn {
                content: request.content,
                title: request.title,
                cfg,
                expires_in: request.expires_in,
            })
            .await?
        }
    };
    drop(events.send(Event::Started {
        conversation,
        tools: reading_tools,
    }));

    turn.await
}

/// Prepend a `--cfg` JSON override that disables every tool in `tools` before
/// applying `cfg`.
///
/// Because per-tool `enable = true` settings in user or workspace configs take
/// precedence over `conversation.tools.'*'` defaults and merge key-by-key,
/// explicitly disabling every configured tool first ensures that only the tools
/// re-enabled by `cfg` (the persona) remain active.
///
/// # Errors
///
/// Returns an error if any tool name contains `.`, which would be split as a
/// path separator by `--cfg` assignment.
pub(crate) fn turn_cfg(tools: &[String], cfg: Vec<String>) -> Result<Vec<String>, String> {
    if let Some(name) = tools.iter().find(|name| name.contains('.')) {
        return Err(format!(
            "the tool `{name}` can't be disabled, because a `.` in its name splits a `--cfg` key"
        ));
    }

    let mut out = vec![];
    if !tools.is_empty() {
        let disabled: Map<String, Value> = tools
            .iter()
            .map(|name| (name.clone(), json!({ "enable": false })))
            .collect();
        out.push(json!({ "conversation": { "tools": disabled } }).to_string());
    }
    out.extend(cfg);

    Ok(out)
}
