//! Turn scheduling and message routing between the chat transport and the `jp`
//! host.
//!
//! The bridge runs one turn at a time.
//! While a turn is running for a chat, incoming messages from that same chat
//! are injected into the turn as interrupt replies (like `[r] Reply` in the
//! terminal).
//! If the turn has already finished, or if the message is from another chat, it
//! is queued and batched into the chat's next query.
//!
//! Each chat uses one conversation per day, or per week with a weekly
//! `rollover`.
//! Every query passes `--cfg` overrides for the configured persona and the
//! sender's identity so that tickets, RFDs, and wiki pages created during the
//! turn are attributed to the sender.
//!
//! The repository checkout is locked for the duration of each turn and
//! inspected when the turn ends.
//! If a turn modifies or creates any file outside the configured writable
//! directories, the bridge pauses until the working tree is clean again.

use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use chrono::{Local, NaiveDate};
use tokio::sync::{mpsc, watch};

use crate::{
    answer::last_answer,
    checkout::Checkout,
    host::{Host, Level, NewTurn, ReplyError},
    options::{Options, Rollover},
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

/// Queued messages for a single chat awaiting the next turn.
#[derive(Debug)]
struct Waiting {
    chat: Chat,

    /// Key of the most recent sender in `options.people`, used as the turn's
    /// identity.
    person: String,

    texts: Vec<String>,
}

/// State of the currently executing turn.
#[derive(Debug)]
struct Running {
    chat: Chat,

    /// Conversation ID, or `None` while the host is still creating a new
    /// conversation.
    conversation: Option<String>,
}

/// Completion event from a spawned host task.
#[derive(Debug)]
pub(crate) enum Event {
    /// The host created the new conversation for the active turn.
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
    today: Box<dyn Fn() -> NaiveDate + Send>,

    queue: VecDeque<Waiting>,
    running: Option<Running>,

    /// Paths outside the writable directories that paused the bridge.
    paused: Option<Vec<String>>,

    /// Chats that have already been notified about the current pause.
    told: Vec<Chat>,

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
            queue: VecDeque::new(),
            running: None,
            paused: None,
            told: vec![],
            strangers: HashSet::new(),
            events_tx,
            events_rx,
        }
    }

    /// Override the clock used for conversation rollover.
    #[cfg(test)]
    pub(crate) fn with_today(mut self, today: impl Fn() -> NaiveDate + Send + 'static) -> Self {
        self.today = Box::new(today);
        self
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
            Ok(stray) => self.pause(stray),
            Err(error) => self.host.log(
                Level::Error,
                &format!("Failed to check the checkout: {error}"),
            ),
        }

        let mut typing = tokio::time::interval(TYPING_INTERVAL);
        let mut pull = tokio::time::interval(self.options.checkout.pull_interval);

        loop {
            tokio::select! {
                message = inbound.recv() => match message {
                    Some(message) => self.receive(message),
                    None => return Err("The chat transport stopped.".to_owned()),
                },
                Some(event) = self.events_rx.recv() => self.handle(event).await,
                _ = typing.tick() => self.renew_typing(),
                _ = pull.tick() => self.idle(),
                _ = shutdown.changed() => return Ok(()),
            }

            self.advance();
        }
    }

    pub(crate) fn receive(&mut self, message: Inbound) {
        let Some((key, person)) = self.options.signal_person(&message.sender) else {
            // Omit the message body from the log to avoid recording untrusted or private text.
            self.host.log(
                Level::Warn,
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

        if self.paused.is_some() {
            if !self.told.contains(&message.chat) {
                self.outbox.send(&message.chat, PAUSED);
                self.told.push(message.chat.clone());
            }
            self.wait(message.chat, key, text);
            return;
        }

        let conversation = self
            .running
            .as_ref()
            .filter(|running| running.chat == message.chat)
            .and_then(|running| running.conversation.clone());
        match conversation {
            Some(conversation) => self.reply(message.chat, key, text, &conversation),
            None => self.wait(message.chat, key, text),
        }
    }

    /// Deliver `text` as an interrupt reply to the active turn on
    /// `conversation`.
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

    /// Append `text` to the pending message batch for `chat`.
    fn wait(&mut self, chat: Chat, person: String, text: String) {
        match self.queue.iter_mut().find(|waiting| waiting.chat == chat) {
            Some(waiting) => {
                waiting.person = person;
                waiting.texts.push(text);
            }
            None => self.queue.push_back(Waiting {
                chat,
                person,
                texts: vec![text],
            }),
        }
    }

    #[cfg(test)]
    pub(crate) async fn next_event(&mut self) -> Event {
        self.events_rx
            .recv()
            .await
            .expect("the bridge holds a sender")
    }

    pub(crate) async fn handle(&mut self, event: Event) {
        match event {
            Event::Started { conversation } => {
                let period = self.period();
                let Some(running) = &mut self.running else {
                    return;
                };
                if let Err(error) = self.state.set(&running.chat, period, &conversation) {
                    self.host.log(Level::Error, &error);
                }
                running.conversation = Some(conversation);
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
            } => match result {
                Ok(()) => {}
                Err(ReplyError::Refused(_)) => self.wait(chat, person, text),
                Err(ReplyError::Broken(error)) => {
                    self.host.log(
                        Level::Error,
                        &format!("Failed to hand over a reply: {error}"),
                    );
                    self.wait(chat, person, text);
                }
            },
        }
    }

    /// Read the completed turn's final response from the host and send it to
    /// the chat.
    async fn answer(&self, running: &Running, result: Result<(), String>) {
        if let Err(error) = result {
            self.host.log(
                Level::Error,
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
                        Level::Info,
                        &format!("The turn for {} chose not to answer.", running.chat),
                    );
                }
                answer => self
                    .outbox
                    .send(&running.chat, answer.as_deref().unwrap_or(NO_ANSWER)),
            },
            Err(error) => {
                self.host.log(
                    Level::Error,
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
            Ok(stray) => self.pause(stray),
            Err(error) => {
                self.host.log(
                    Level::Error,
                    &format!("Failed to check the checkout: {error}"),
                );
                self.pause(vec![format!("(the check failed: {error})")]);
            }
        }
    }

    /// Pause turn execution and alert the admin with the out-of-scope paths.
    fn pause(&mut self, stray: Vec<String>) {
        self.alert(&format!(
            "The bridge is paused: these changes are outside the writable directories, so take or \
             discard them before it starts another turn:\n{}",
            stray
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        self.paused = Some(stray);
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
                    self.told.clear();
                    self.alert("The tree is clean again; the bridge has resumed.");
                }
                Ok(false) => return,
                Err(error) => {
                    self.host.log(
                        Level::Error,
                        &format!("Failed to check the checkout: {error}"),
                    );
                    return;
                }
            }
        }

        let Some(waiting) = self.queue.pop_front() else {
            return;
        };
        match self.checkout.begin() {
            Ok(None) => {}
            Ok(Some(warning)) => self.host.log(Level::Warn, &warning),
            Err(error) => {
                self.alert(&format!("A turn couldn't start: {error}"));
                self.queue.push_front(waiting);
                return;
            }
        }

        self.start(waiting);
    }

    /// Spawn a turn on the current conversation for `waiting.chat`, creating
    /// the conversation if needed.
    fn start(&mut self, waiting: Waiting) {
        let period = self.period();
        let content = waiting.texts.join("\n\n");
        let cfg = self.cfg(&waiting.person);
        let events = self.events_tx.clone();

        self.outbox.typing(&waiting.chat, true);

        let conversation = self
            .state
            .conversation(&waiting.chat, period)
            .map(ToOwned::to_owned);
        if let Some(conversation) = conversation {
            let turn = self.host.query(&conversation, &content, cfg);
            tokio::spawn(async move {
                drop(events.send(Event::Ended { result: turn.await }));
            });

            self.running = Some(Running {
                chat: waiting.chat,
                conversation: Some(conversation),
            });
            return;
        }

        let new = NewTurn {
            content,
            title: self.title(&waiting, period),
            cfg,
            expires_in: self.options.conversation_ttl.clone(),
        };
        let host = self.host.clone();
        tokio::spawn(async move {
            let result = match host.start(new).await {
                Ok((conversation, turn)) => {
                    drop(events.send(Event::Started { conversation }));
                    turn.await
                }
                Err(error) => Err(error),
            };
            drop(events.send(Event::Ended { result }));
        });

        self.running = Some(Running {
            chat: waiting.chat,
            conversation: None,
        });
    }

    /// Build the `--cfg` arguments for a turn triggered by `person`.
    ///
    /// Applies the configured persona, sets `user.name` and the plugin
    /// `assistant` attribution to `<Name> via jp`, and forces
    /// `conversation.start_local = true` so conversation files are stored
    /// outside the workspace checkout.
    fn cfg(&self, person: &str) -> Vec<String> {
        let name = self
            .options
            .people
            .get(person)
            .map_or(person, |person| person.name.as_str());
        let author = format!("{name} via jp");

        let mut cfg: Vec<String> = self.options.persona.iter().cloned().collect();
        cfg.extend(self.options.cfg.iter().cloned());
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

    fn title(&self, waiting: &Waiting, period: NaiveDate) -> String {
        let when = match self.options.rollover {
            Rollover::Daily => period.to_string(),
            Rollover::Weekly(_) => format!("week of {period}"),
        };

        match waiting.chat {
            Chat::Direct(_) => {
                let name = self
                    .options
                    .people
                    .get(&waiting.person)
                    .map_or(waiting.person.as_str(), |person| person.name.as_str());
                format!("Chat with {name}, {when}")
            }
            Chat::Group(_) => format!("Group chat, {when}"),
        }
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
            self.host.log(Level::Warn, &warning);
        }
    }

    /// Log `text`, print it to the host, and DM the configured admin.
    fn alert(&self, text: &str) {
        self.host.log(Level::Warn, text);
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
