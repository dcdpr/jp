//! Turn scheduling and message routing between the chat transport and the `jp`
//! host.
//!
//! The bridge runs one turn at a time, as the person whose message started it.
//! While a turn is running, a message from that person in that chat is injected
//! into the turn as an interrupt reply (like `[r] Reply` in the terminal), one
//! reply at a time, and never ahead of an earlier message from the same chat.
//! Any other message waits in the [`Queue`].
//! Before either, a message can wait in the [`Inbox`] for a model to decide
//! what to do with it: answer it, ignore it, or, for a message that may give a
//! command, start a new conversation or compact the current one.
//!
//! Each chat uses one conversation per day, or per week with a weekly
//! `rollover`.
//! Every query passes `--cfg` overrides that disable every configured tool,
//! then apply the configured persona, which enables the tools it names, then
//! the sender's identity so that tickets, RFDs, and wiki pages created during
//! the turn are attributed to the sender.
//! If the bridge stops while a turn is running, it waits for the turn to finish
//! and verifies the working tree before releasing the checkout lock.
//!
//! The repository checkout is locked for the duration of each turn and
//! inspected when the turn ends.
//! If a turn modifies or creates any file outside the configured writable
//! directories, the bridge pauses until the working tree is clean again.

use std::{sync::Arc, time::Duration};

use chrono::NaiveDate;
use jp_plugin::message::{Compaction, StoredCompaction, SummarySource, ToolCallsMode};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, watch};

use crate::{
    answer::last_answer,
    checkout::Checkout,
    decide::{Action, Decide, Verdict},
    host::{Host, Level, NewTurn, ReplyError},
    inbox::{Inbox, Incoming, Next},
    options::{Options, Rollover},
    queue::{Queue, Waiting},
    state::{Conversations, State},
    transport::{Attachment, Chat, Inbound, Outbox},
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

const NEW_CONVERSATION: &str = "Your next message starts a new conversation.";

const NOTHING_TO_COMPACT: &str = "There's no conversation to compact yet.";

const BUSY_COMPACTING: &str = "I'm still answering here. Ask me to compact once I've replied.";

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

    /// What to do with the first message in the inbox.
    Decided {
        result: Result<Verdict, String>,
    },

    /// Compaction of `chat`'s conversation completed.
    Compacted {
        chat: Chat,
        result: Result<Vec<StoredCompaction>, String>,
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
    conversations: Conversations,
    queue: Queue,
    running: Option<Running>,

    /// Chats already notified of the pause while out-of-scope changes are
    /// present.
    paused: Option<Vec<Chat>>,

    inbox: Inbox,

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
        let conversations = Conversations::new(state, options.rollover);

        Self {
            host,
            outbox,
            checkout,
            options,
            conversations,
            queue: Queue::default(),
            running: None,
            paused: None,
            inbox: Inbox::default(),
            events_tx,
            events_rx,
        }
    }

    /// Configure the [`Decide`] client used to classify incoming messages.
    pub(crate) fn with_decider(mut self, decider: Option<Arc<dyn Decide>>) -> Self {
        self.inbox.decide_with(decider);
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
                _ = typing.tick() => {
                    if let Some(running) = &self.running {
                        self.outbox.typing(&running.chat, true);
                    }
                }
                _ = pull.tick() => self.idle(),
                _ = shutdown.changed() => {
                    self.finish(true).await;
                    return Ok(());
                }
            }

            self.advance();
        }
    }

    /// Wait for the active turn to finish before exiting.
    ///
    /// Because the host continues running a delegated turn after the plugin
    /// exits, exiting early would release the checkout lock while the turn is
    /// still writing and skip the post-turn working-tree check.
    /// When `stop` is `true`, sends a stop interrupt to the active turn first.
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
                && self.inbox.first_sighting(id)
            {
                self.host.print(&stranger(&message, id));
            }
            return;
        };
        let key = key.to_owned();
        let Some(text) = content(&message, &person.name) else {
            return;
        };

        // Commands require a leading @mention in group chats; in direct chats
        // every message is eligible.
        let commands = message.leading_mention || matches!(message.chat, Chat::Direct(_));
        self.inbox.push(message.chat, key, text, commands);
        self.pump();
    }

    /// Drain ready messages from the inbox, or spawn a decision task for the
    /// front message.
    fn pump(&mut self) {
        while let Some(next) = self.inbox.next() {
            match next {
                Next::Take(incoming) => self.route(incoming),
                Next::Decide(decision) => {
                    let events = self.events_tx.clone();
                    tokio::spawn(async move {
                        drop(events.send(Event::Decided {
                            result: decision.await,
                        }));
                    });
                }
            }
        }
    }

    /// Apply the decision for the front inbox message and advance the inbox.
    fn decided(&mut self, result: Result<Verdict, String>) {
        if let Err(error) = &result {
            self.host.log(
                Level::WARN,
                &format!("The decide API failed, so the message is answered: {error}"),
            );
        }
        let threshold = self
            .options
            .decide
            .as_ref()
            .map_or(0.0, |decide| decide.threshold);

        if let Some(incoming) = self.inbox.decided() {
            match action(result.ok(), threshold) {
                Action::Answer => self.route(incoming),
                Action::Ignore => self.host.log(
                    Level::INFO,
                    &format!("A message in {} isn't for the assistant.", incoming.chat),
                ),
                Action::NewConversation => {
                    if let Err(error) = self.conversations.forget(&incoming.chat) {
                        self.host.log(Level::ERROR, &error);
                    }
                    self.outbox.send(&incoming.chat, NEW_CONVERSATION);
                }
                Action::Compact => {
                    // The running turn holds its conversation; compacting it
                    // now would find it locked.
                    let busy = self
                        .running
                        .as_ref()
                        .is_some_and(|running| running.chat == incoming.chat);
                    match self.conversations.current(&incoming.chat) {
                        None => self.outbox.send(&incoming.chat, NOTHING_TO_COMPACT),
                        Some(_) if busy => self.outbox.send(&incoming.chat, BUSY_COMPACTING),
                        Some(conversation) => {
                            // No turn starts while the conversation is compacted.
                            self.queue.hold();
                            compact(
                                &self.host,
                                &self.events_tx,
                                incoming.chat,
                                conversation,
                                self.options.compact.clone(),
                            );
                        }
                    }
                }
            }
        }
        self.pump();
    }

    /// Hand `incoming` to the running turn, or queue it for a turn of its own.
    fn route(&mut self, incoming: Incoming) {
        let Incoming {
            chat, person, text, ..
        } = incoming;

        if let Some(told) = &mut self.paused {
            if !told.contains(&chat) {
                self.outbox.send(&chat, PAUSED);
                told.push(chat.clone());
            }
            self.queue.push(chat, person, text);
            return;
        }

        // Only the person the turn runs as, so what it writes stays theirs.
        let conversation = self
            .running
            .as_ref()
            .filter(|running| running.chat == chat && running.person == person)
            .and_then(|running| running.conversation.clone());
        match conversation {
            Some(conversation) if self.queue.try_reply(&chat) => {
                self.reply(chat, person, text, &conversation);
            }
            _ => self.queue.push(chat, person, text),
        }
    }

    /// Spawn a task to deliver `text` as an in-turn reply on `conversation`.
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

    /// Send the next queued reply batch from the active turn's sender, if any.
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
                let Some(running) = &mut self.running else {
                    return;
                };
                running.conversation = Some(conversation.clone());
                let chat = running.chat.clone();

                if let Err(error) = self.conversations.record(&chat, &conversation) {
                    self.host.log(Level::ERROR, &error);
                }
                self.hand_over();
            }

            Event::Ended { result } => {
                let Some(running) = self.running.take() else {
                    return;
                };
                self.outbox.typing(&running.chat, false);
                if let Some(sent) = self.answer(&running, result).await {
                    self.inbox
                        .remember(&running.chat, &format!("Assistant: {sent}"));
                }
                self.after_turn();
            }

            Event::Decided { result } => self.decided(result),

            Event::Compacted { chat, result } => self.compacted(&chat, result),

            Event::Replied {
                chat,
                person,
                text,
                result,
            } => {
                self.queue.replied();
                match result {
                    Ok(()) => self.hand_over(),
                    // The turn ended before accepting the reply; requeue it at
                    // the front of the chat's queue.
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

    /// Release the queue hold after compaction and report the result to `chat`.
    fn compacted(&mut self, chat: &Chat, result: Result<Vec<StoredCompaction>, String>) {
        self.queue.release();
        let reply = match result {
            Ok(stored) => describe_compactions(&stored),
            Err(error) => {
                self.host.log(
                    Level::ERROR,
                    &format!("Compacting the conversation in {chat} failed: {error}"),
                );
                format!("I couldn't compact this conversation: {error}")
            }
        };
        self.outbox.send(chat, &reply);
    }

    /// Read the completed turn's final response from the host and send it to
    /// the chat, returning what the assistant said, if anything.
    async fn answer(&self, running: &Running, result: Result<(), String>) -> Option<String> {
        if let Err(error) = result {
            self.host.log(
                Level::ERROR,
                &format!("A turn for {} failed: {error}", running.chat),
            );
            self.outbox.send(&running.chat, FAILED);
            return None;
        }

        let conversation = running.conversation.as_ref()?;
        match self.host.events(conversation).await {
            Ok(events) => match last_answer(&events) {
                Some(answer) if is_silent(&answer) => {
                    self.host.log(
                        Level::INFO,
                        &format!("The turn for {} chose not to answer.", running.chat),
                    );
                    None
                }
                answer => {
                    let answer = answer.unwrap_or_else(|| NO_ANSWER.to_owned());
                    self.outbox.send(&running.chat, &answer);
                    Some(answer)
                }
            },
            Err(error) => {
                self.host.log(
                    Level::ERROR,
                    &format!("Failed to read conversation {conversation}: {error}"),
                );
                self.outbox.send(&running.chat, FAILED);
                None
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
        let conversation = self
            .conversations
            .current(&waiting.chat)
            .map(ToOwned::to_owned);
        let request = TurnRequest {
            content: waiting.texts.join("\n\n"),
            title: title(
                &self.options,
                self.conversations.rollover(),
                &waiting,
                self.conversations.period(),
            ),
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

/// Resolve the action to take from the decider's `verdict`.
///
/// Falls back to [`Action::Answer`] when the request failed or the verdict's
/// probability is below `threshold`.
fn action(verdict: Option<Verdict>, threshold: f64) -> Action {
    verdict
        .filter(|verdict| verdict.probability >= threshold)
        .map_or(Action::Answer, |verdict| verdict.action)
}

/// Spawn a task to compact `conversation` on the host and emit
/// [`Event::Compacted`].
fn compact<H: Host>(
    host: &H,
    events: &mpsc::UnboundedSender<Event>,
    chat: Chat,
    conversation: &str,
    compactions: Vec<Compaction>,
) {
    let compaction = host.compact(conversation, compactions);
    let events = events.clone();

    tokio::spawn(async move {
        let result = compaction.await;
        drop(events.send(Event::Compacted { chat, result }));
    });
}

/// Format a user-facing summary of the stored compactions.
fn describe_compactions(stored: &[StoredCompaction]) -> String {
    let mut text = "Compacted this conversation:".to_owned();
    for compaction in stored {
        let turns = if compaction.from == compaction.to {
            format!("turn {}", compaction.from)
        } else {
            format!("turns {} to {}", compaction.from, compaction.to)
        };

        // A summary replaces the whole range, so nothing else applies to it.
        let done = match compaction.summary {
            Some(SummarySource::Generated) => vec!["summarized"],
            Some(SummarySource::Text) => vec!["replaced by the given summary"],
            None => {
                let mut done = vec![];
                if compaction.reasoning {
                    done.push("reasoning stripped");
                }
                done.extend(compaction.tool_calls.map(|mode| match mode {
                    ToolCallsMode::Strip => "tool calls stripped",
                    ToolCallsMode::StripRequests => "tool call arguments stripped",
                    ToolCallsMode::StripResponses => "tool call results stripped",
                    ToolCallsMode::Omit => "tool calls removed",
                }));
                done
            }
        };

        text.push_str(&format!("\n- {turns}: {}", done.join(", ")));
    }

    text
}

/// Format the prompt text for `message` from `name`, appending attachment
/// notices and prefixing the sender's name in group chats.
///
/// Returns `None` if the message has neither text nor attachments.
fn content(message: &Inbound, name: &str) -> Option<String> {
    let mut parts: Vec<String> = vec![];
    let text = message.text.trim();
    if !text.is_empty() {
        parts.push(text.to_owned());
    }
    parts.extend(message.attachments.iter().map(describe));
    if parts.is_empty() {
        return None;
    }

    let body = parts.join("\n");
    Some(match message.chat {
        Chat::Group(_) if message.addressed => format!("{name} (to you): {body}"),
        Chat::Group(_) => format!("{name}: {body}"),
        Chat::Direct(_) => body,
    })
}

/// Format an attachment notice with its MIME type and local path.
///
/// The planner persona cannot read arbitrary filesystem paths, but can record
/// the path in a ticket so a maintainer can retrieve the file from the bridge
/// host.
fn describe(attachment: &Attachment) -> String {
    let what = match &attachment.name {
        Some(name) => format!("{name} ({})", attachment.content_type),
        None => attachment.content_type.clone(),
    };

    match &attachment.path {
        Some(path) => format!("[Attached {what}, saved on the bridge's machine at {path}]"),
        None => format!("[Attached {what}, which the bridge couldn't save]"),
    }
}

fn check_failed(error: &str) -> String {
    format!("Failed to check the checkout: {error}")
}

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
fn title(options: &Options, rollover: Rollover, waiting: &Waiting, period: NaiveDate) -> String {
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
struct TurnRequest {
    content: String,
    title: String,

    /// [`cfg()`], before [`turn_cfg`] prepends the tool-disabling override.
    cfg: Vec<String>,

    expires_in: String,
}

/// Run a turn on `conversation` (or create a new conversation when `None`) and
/// await its completion.
///
/// Emits [`Event::Started`] once the host accepts the request.
/// Fails before starting the turn if the configured tool list cannot be read,
/// so a turn never runs without first disabling unapproved tools.
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
