//! Turn scheduling and message routing between the chat transport and the `jp`
//! host.
//!
//! The bridge runs one turn at a time, as the person whose message started it.
//! While a turn is running, a message from that person in that chat is injected
//! into the turn as an interrupt reply (like `[r] Reply` in the terminal), one
//! reply at a time, and never ahead of an earlier message from the same chat.
//! Any other message waits in the [`Queue`].
//!
//! Each chat uses one conversation per day, or per week with a weekly
//! `rollover`.
//! Every query passes `--cfg` overrides that disable every configured tool,
//! then apply the configured persona, which enables the tools it names, then
//! the sender's identity so that tickets, RFDs, and wiki pages created during
//! the turn are attributed to the sender.
//! When the bridge stops while a turn runs, it waits for the turn to end, and
//! checks what it wrote, before letting go of the checkout.
//!
//! The repository checkout is locked for the duration of each turn and
//! inspected when the turn ends.
//! If a turn modifies or creates any file outside the configured writable
//! directories, the bridge pauses until the working tree is clean again.

use std::{collections::HashSet, sync::Arc, time::Duration};

use chrono::{Local, NaiveDate};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, watch};

use crate::{
    answer::last_answer,
    checkout::Checkout,
    host::{Host, Level, NewTurn, ReplyError},
    options::{Options, Rollover},
    queue::{Queue, Waiting},
    state::State,
    transport::{Chat, Inbound, Outbox},
};

/// How often the typing indicator is renewed while a turn runs.
///
/// Signal clears it after 15 seconds.
const TYPING_INTERVAL: Duration = Duration::from_secs(10);

const NO_ANSWER: &str = "(No answer.)";

/// Sentinel token the assistant outputs to suppress a reply (for example, when
/// a group message is not addressed to the assistant).
const SILENT: &str = "NO_REPLY";

/// User-facing message sent when a turn fails; the underlying error is logged.
const FAILED: &str = "Sorry, that didn't work. The bridge's log says why.";

const TEXT_ONLY: &str = "I can only read text, so I didn't get the attachment.";

const PAUSED: &str = "Changes are paused until the last ones have been reviewed. I'll answer as \
                      soon as that's done.";

/// State of the currently executing turn.
#[derive(Debug)]
struct Running {
    chat: Chat,

    /// Key in `options.people` of the person the turn runs as.
    person: String,

    /// Conversation ID, or `None` until the host has the turn's request.
    conversation: Option<String>,
}

/// Completion event from a spawned host task.
#[derive(Debug)]
pub(crate) enum Event {
    /// The host has the active turn's request, on this conversation.
    Started {
        conversation: String,
    },

    Ended {
        result: Result<(), String>,
    },

    /// Outcome of delivering an in-turn reply to the host.
    Replied {
        chat: Chat,
        person: String,
        text: String,
        result: Result<(), ReplyError>,
    },
}

pub(crate) struct Bridge<H: Host, C: Checkout> {
    host: H,
    outbox: Arc<dyn Outbox>,
    checkout: C,
    options: Options,
    state: State,

    /// The clock that decides which rollover period a turn falls in.
    today: Box<dyn Fn() -> NaiveDate + Send + Sync>,

    queue: Queue,
    running: Option<Running>,

    /// While paused by changes outside the writable directories, the chats
    /// already told so.
    paused: Option<Vec<Chat>>,

    /// Primary sender IDs of unconfigured senders already printed during this
    /// run, so each unknown sender is reported at most once.
    strangers: HashSet<String>,

    events_tx: mpsc::UnboundedSender<Event>,
    events_rx: mpsc::UnboundedReceiver<Event>,
}

impl<H: Host, C: Checkout> Bridge<H, C> {
    pub(crate) fn new(
        host: H,
        outbox: Arc<dyn Outbox>,
        checkout: C,
        options: Options,
        state: State,
    ) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        Self {
            host,
            outbox,
            checkout,
            options,
            state,
            today: Box::new(|| Local::now().date_naive()),
            queue: Queue::default(),
            running: None,
            paused: None,
            strangers: HashSet::new(),
            events_tx,
            events_rx,
        }
    }

    /// Process inbound messages and turn events until shutdown or transport
    /// exit.
    ///
    /// Returns `Err` if the transport channel closes unexpectedly.
    pub(crate) async fn run(
        mut self,
        mut inbound: mpsc::Receiver<Inbound>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), String> {
        match self.checkout.stray() {
            Ok(stray) if stray.is_empty() => {}
            Ok(stray) => self.pause(&stray),
            Err(error) => self.host.log(Level::ERROR, &check_failed(&error)),
        }

        let mut typing = tokio::time::interval(TYPING_INTERVAL);
        let mut pull = tokio::time::interval(self.options.checkout.pull_interval);

        loop {
            tokio::select! {
                message = inbound.recv() => {
                    let Some(message) = message else {
                        self.finish(false).await;
                        return Err("The chat transport stopped.".to_owned());
                    };
                    self.receive(message);
                }
                Some(event) = self.events_rx.recv() => self.handle(event).await,
                _ = typing.tick() => self.renew_typing(),
                _ = pull.tick() => self.idle(),
                _ = shutdown.changed() => {
                    self.finish(true).await;
                    return Ok(());
                }
            }

            self.advance();
        }
    }

    /// Wait for the running turn to end before the bridge exits.
    ///
    /// The host keeps running a turn after the plugin that asked for it exits,
    /// so exiting first would release the checkout's lock while the turn still
    /// writes, and skip the check of what it wrote.
    /// With `stop`, the turn is asked to stop; otherwise it runs to its end.
    pub(crate) async fn finish(&mut self, stop: bool) {
        self.queue.close();
        if self.running.is_some() {
            self.host.log(
                Level::INFO,
                "Waiting for the running turn to end before exiting.",
            );
        }

        let mut stopping = false;
        while let Some(running) = &self.running {
            if stop
                && !stopping
                && let Some(conversation) = &running.conversation
            {
                stopping = true;
                let request = self.host.stop(conversation);
                let host = self.host.clone();
                tokio::spawn(async move {
                    if let Err(error) = request.await {
                        host.log(
                            Level::WARN,
                            &format!("Failed to stop the running turn: {error}"),
                        );
                    }
                });
            }

            let Some(event) = self.events_rx.recv().await else {
                break;
            };
            self.handle(event).await;
        }
    }

    pub(crate) fn receive(&mut self, message: Inbound) {
        let Some((key, person)) = self.options.signal_person(&message.sender) else {
            // Omit the message body from the log to avoid recording untrusted or private text.
            self.host.log(
                Level::WARN,
                &format!(
                    "Dropped a message from {} in {}: not one of the configured people.",
                    message.sender.join(" / "),
                    message.chat
                ),
            );
            if let Some(id) = message.sender.first()
                && self.strangers.insert(id.clone())
            {
                self.host.print(&stranger(&message, id));
            }
            return;
        };
        let (key, name) = (key.to_owned(), person.name.clone());

        if message.attachments > 0 {
            self.outbox.send(&message.chat, TEXT_ONLY);
        }
        let text = message.text.trim();
        if text.is_empty() {
            return;
        }
        let text = match message.chat {
            Chat::Group(_) if message.addressed => format!("{name} (to you): {text}"),
            Chat::Group(_) => format!("{name}: {text}"),
            Chat::Direct(_) => text.to_owned(),
        };

        if let Some(told) = &mut self.paused {
            if !told.contains(&message.chat) {
                self.outbox.send(&message.chat, PAUSED);
                told.push(message.chat.clone());
            }
            self.queue.push(message.chat, key, text);
            return;
        }

        // Only the person the turn runs as, so what it writes stays theirs.
        let conversation = self
            .running
            .as_ref()
            .filter(|running| running.chat == message.chat && running.person == key)
            .and_then(|running| running.conversation.clone());
        match conversation {
            Some(conversation) if self.queue.try_reply(&message.chat) => {
                self.reply(message.chat, key, text, &conversation);
            }
            _ => self.queue.push(message.chat, key, text),
        }
    }

    /// Deliver `text` as an interrupt reply to the active turn on
    /// `conversation`, as the reply the queue has in flight.
    fn reply(&self, chat: Chat, person: String, text: String, conversation: &str) {
        let reply = self.host.reply(conversation, &text);
        let events = self.events_tx.clone();

        tokio::spawn(async move {
            let result = reply.await;
            drop(events.send(Event::Replied {
                chat,
                person,
                text,
                result,
            }));
        });
    }

    /// Hand the running turn the next batch the queue holds for it, if any.
    fn hand_over(&mut self) {
        let Some(running) = &self.running else {
            return;
        };
        let Some(conversation) = running.conversation.clone() else {
            return;
        };
        let Some(waiting) = self.queue.next_reply(&running.chat, &running.person) else {
            return;
        };

        self.reply(
            waiting.chat,
            waiting.person,
            waiting.texts.join("\n\n"),
            &conversation,
        );
    }

    pub(crate) async fn handle(&mut self, event: Event) {
        match event {
            Event::Started { conversation } => {
                let period = self.period();
                let Some(running) = &mut self.running else {
                    return;
                };
                running.conversation = Some(conversation.clone());
                let chat = running.chat.clone();

                if self.state.conversation(&chat, period) != Some(conversation.as_str())
                    && let Err(error) = self.state.set(&chat, period, &conversation)
                {
                    self.host.log(Level::ERROR, &error);
                }
                self.hand_over();
            }

            Event::Ended { result } => {
                let Some(running) = self.running.take() else {
                    return;
                };
                self.outbox.typing(&running.chat, false);
                self.answer(&running, result).await;
                self.after_turn();
            }

            Event::Replied {
                chat,
                person,
                text,
                result,
            } => {
                self.queue.replied();
                match result {
                    Ok(()) => self.hand_over(),
                    // The turn ended before taking it, so it waits for the next
                    // one, ahead of what came after it.
                    Err(ReplyError::Refused(_)) => self.queue.push_first(chat, person, text),
                    Err(ReplyError::Broken(error)) => {
                        self.host.log(
                            Level::ERROR,
                            &format!("Failed to hand over a reply: {error}"),
                        );
                        self.queue.push_first(chat, person, text);
                    }
                }
            }
        }
    }

    /// Read the completed turn's final response from the host and send it to
    /// the chat.
    async fn answer(&self, running: &Running, result: Result<(), String>) {
        if let Err(error) = result {
            self.host.log(
                Level::ERROR,
                &format!("A turn for {} failed: {error}", running.chat),
            );
            self.outbox.send(&running.chat, FAILED);
            return;
        }

        let Some(conversation) = &running.conversation else {
            return;
        };
        match self.host.events(conversation).await {
            Ok(events) => match last_answer(&events) {
                Some(answer) if is_silent(&answer) => {
                    self.host.log(
                        Level::INFO,
                        &format!("The turn for {} chose not to answer.", running.chat),
                    );
                }
                answer => self
                    .outbox
                    .send(&running.chat, answer.as_deref().unwrap_or(NO_ANSWER)),
            },
            Err(error) => {
                self.host.log(
                    Level::ERROR,
                    &format!("Failed to read conversation {conversation}: {error}"),
                );
                self.outbox.send(&running.chat, FAILED);
            }
        }
    }

    /// Release the checkout lock and pause the bridge if the turn modified
    /// paths outside `checkout.writable`.
    fn after_turn(&mut self) {
        match self.checkout.end() {
            Ok(stray) if stray.is_empty() => {}
            Ok(stray) => self.pause(&stray),
            Err(error) => {
                self.host.log(Level::ERROR, &check_failed(&error));
                self.pause(&[format!("(the check failed: {error})")]);
            }
        }
    }

    /// Pause turn execution and alert the admin with the out-of-scope paths.
    fn pause(&mut self, stray: &[String]) {
        self.alert(&format!(
            "The bridge is paused: these changes are outside the writable directories, so take or \
             discard them before it starts another turn:\n{}",
            stray
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        self.paused.get_or_insert_with(Vec::new);
    }

    /// Start the next queued turn if no turn is running and the bridge is not
    /// paused.
    pub(crate) fn advance(&mut self) {
        if self.running.is_some() {
            return;
        }
        if self.paused.is_some() {
            match self.checkout.is_clean() {
                Ok(true) => {
                    self.paused = None;
                    self.alert("The tree is clean again; the bridge has resumed.");
                }
                Ok(false) => return,
                Err(error) => {
                    self.host.log(Level::ERROR, &check_failed(&error));
                    return;
                }
            }
        }

        let Some(waiting) = self.queue.next_turn() else {
            return;
        };
        match self.checkout.begin() {
            Ok(None) => {}
            Ok(Some(warning)) => self.host.log(Level::WARN, &warning),
            Err(error) => {
                self.alert(&format!("A turn couldn't start: {error}"));
                self.queue.restore(waiting);
                return;
            }
        }

        self.start(waiting);
    }

    /// Spawn a turn on the current conversation for `waiting.chat`, creating
    /// the conversation if needed.
    fn start(&mut self, waiting: Waiting) {
        let period = self.period();
        let conversation = self
            .state
            .conversation(&waiting.chat, period)
            .map(ToOwned::to_owned);
        let request = TurnRequest {
            content: waiting.texts.join("\n\n"),
            title: title(&self.options, &waiting, period),
            cfg: cfg(&self.options, &waiting.person),
            expires_in: self.options.conversation_ttl.clone(),
        };
        let host = self.host.clone();
        let events = self.events_tx.clone();

        self.outbox.typing(&waiting.chat, true);
        self.running = Some(Running {
            chat: waiting.chat,
            person: waiting.person,
            conversation: None,
        });

        tokio::spawn(async move {
            let result = run_turn(&host, conversation, request, &events).await;
            drop(events.send(Event::Ended { result }));
        });
    }

    /// Start date of the current rollover period.
    fn period(&self) -> NaiveDate {
        self.options.rollover.start((self.today)())
    }

    fn renew_typing(&self) {
        if let Some(running) = &self.running {
            self.outbox.typing(&running.chat, true);
        }
    }

    fn idle(&mut self) {
        if self.running.is_some() {
            return;
        }
        if let Some(warning) = self.checkout.idle() {
            self.host.log(Level::WARN, &warning);
        }
    }

    /// Log `text`, print it to the host, and DM the configured admin.
    fn alert(&self, text: &str) {
        self.host.log(Level::WARN, text);
        self.host.print(text);

        let account = self
            .options
            .admin
            .as_ref()
            .and_then(|admin| self.options.people.get(admin))
            .and_then(|admin| admin.signal.clone());
        if let Some(account) = account {
            self.outbox.send(&Chat::Direct(account), text);
        }
    }
}

/// The log line for a checkout whose state couldn't be read.
fn check_failed(error: &str) -> String {
    format!("Failed to check the checkout: {error}")
}

/// Build the `--cfg` arguments for a turn triggered by `person`, which
/// [`turn_cfg`] puts after the entry disabling every tool.
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

/// Title for the conversation a turn for `waiting` creates in the period
/// starting on `period`.
fn title(options: &Options, waiting: &Waiting, period: NaiveDate) -> String {
    let when = match options.rollover {
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

/// What a turn asks the host for, besides the conversation it runs on.
struct TurnRequest {
    content: String,

    /// Title for a conversation the turn creates.
    title: String,

    /// [`cfg`], before [`turn_cfg`] adds the tool entry.
    cfg: Vec<String>,

    /// Expiry for a conversation the turn creates.
    expires_in: String,
}

/// Start a turn on `conversation`, or on a new one when it is `None`, and wait
/// for it to end.
///
/// Sends [`Event::Started`] once the host has the request.
/// A turn whose tool list can't be read doesn't start: running it without
/// disabling the configured tools would let them through.
async fn run_turn<H: Host>(
    host: &H,
    conversation: Option<String>,
    request: TurnRequest,
    events: &mpsc::UnboundedSender<Event>,
) -> Result<(), String> {
    let tools = host
        .tools()
        .await
        .map_err(|error| format!("couldn't read the configured tools: {error}"))?;
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
    drop(events.send(Event::Started { conversation }));

    turn.await
}

/// The `--cfg` arguments for a turn: an entry disabling every one of `tools`,
/// then `cfg`.
///
/// A tool's own `enable` wins over the `*` defaults, and the per-tool table
/// merges key by key across layers, so a tool a user or workspace config
/// enables stays enabled under a persona that doesn't name it.
/// Disabling every configured tool first leaves enabled exactly the tools that
/// `cfg` enables by name.
///
/// # Errors
///
/// A tool name holding a `.` can't be addressed by a `--cfg` key, so it can't
/// be disabled, and the turn is refused.
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

/// Return whether `answer` matches the [`SILENT`] (`NO_REPLY`) sentinel.
///
/// Tolerates surrounding whitespace, quotes, backticks, and a trailing period
/// in case the model wraps the token.
fn is_silent(answer: &str) -> bool {
    answer
        .trim()
        .trim_matches(['`', '"', '\''])
        .trim_end_matches('.')
        == SILENT
}

/// Format the notice and TOML allowlist snippet printed the first time an
/// unconfigured sender writes to the bridge.
fn stranger(message: &Inbound, id: &str) -> String {
    let name = message
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Unknown");

    // JSON string escaping is valid in a TOML basic string.
    let quote = |text: &str| serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""));

    format!(
        "{name} ({id}) wrote in {chat}, and isn't configured, so the bridge doesn't answer them. \
         To answer them, add this to the bridge's configuration and restart \
         it:\n\n[plugins.command.serve-chat.options.people.{key}]\nname = {name_value}\nsignal = \
         {id_value}\n",
        chat = message.chat,
        key = person_key(name),
        name_value = quote(name),
        id_value = quote(id),
    )
}

/// Derive a TOML table key from `name` by lowercasing ASCII alphanumerics and
/// collapsing runs of other characters to `-`.
fn person_key(name: &str) -> String {
    let mut key = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c.to_ascii_lowercase());
        } else if !key.is_empty() && !key.ends_with('-') {
            key.push('-');
        }
    }
    let key = key.trim_end_matches('-');

    if key.is_empty() {
        "person".to_owned()
    } else {
        key.to_owned()
    }
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
