use std::{
    collections::BTreeSet,
    env,
    future::{self, Future},
    sync::Mutex,
};

use camino::Utf8PathBuf;
use chrono::{NaiveDate, Weekday};
use jp_config::{
    AppConfig, PartialConfig as _,
    assignment::{AssignKeyValue as _, KvAssignment},
    util::{build, load_partial_at_path},
};
use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

use super::*;
use crate::{
    decide::{Action, Decision, Verdict},
    host::{NewTurn, Turn, tool_names},
    options::{DecideOptions, Rollover},
    turn::turn_cfg,
};

struct HostLog {
    calls: Vec<String>,
    turns: Vec<oneshot::Sender<Result<(), String>>>,
    conversations: u32,
    refuse_replies: bool,

    /// Each log event's message and fields.
    logs: Vec<(String, Map<String, Value>)>,
    prints: Vec<String>,

    /// Custom response text returned instead of `"Answer from
    /// <conversation>."`.
    answer: Option<String>,
    cfgs: Vec<Vec<String>>,
    tools: Result<Vec<String>, String>,
    tools_delay: Duration,
    compactions: Vec<oneshot::Sender<Result<Vec<StoredCompaction>, String>>>,
    compacted: Vec<Vec<Compaction>>,
}

impl Default for HostLog {
    fn default() -> Self {
        Self {
            calls: vec![],
            turns: vec![],
            conversations: 0,
            refuse_replies: false,
            logs: vec![],
            prints: vec![],
            answer: None,
            cfgs: vec![],
            tools: Ok(vec![]),
            tools_delay: Duration::ZERO,
            compactions: vec![],
            compacted: vec![],
        }
    }
}

/// Fake [`Host`] whose turns complete when triggered by the test and whose
/// conversations return `"Answer from <conversation>."`.
#[derive(Clone, Default)]
struct FakeHost(Arc<Mutex<HostLog>>);

impl FakeHost {
    fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().calls.clone()
    }

    fn logs(&self) -> Vec<String> {
        let log = self.0.lock().unwrap();
        log.logs
            .iter()
            .map(|(message, _)| message.clone())
            .collect()
    }

    fn logged(&self, message: &str) -> Vec<Value> {
        let log = self.0.lock().unwrap();
        log.logs
            .iter()
            .filter(|(logged, _)| logged == message)
            .map(|(_, fields)| Value::Object(fields.clone()))
            .collect()
    }

    fn end_turn(&self, result: Result<(), String>) {
        let turn = self.0.lock().unwrap().turns.remove(0);
        turn.send(result).unwrap();
    }

    fn prints(&self) -> Vec<String> {
        self.0.lock().unwrap().prints.clone()
    }

    fn cfgs(&self) -> Vec<Vec<String>> {
        self.0.lock().unwrap().cfgs.clone()
    }

    fn set_tools(&self, tools: Result<Vec<String>, String>) {
        self.0.lock().unwrap().tools = tools;
    }

    fn delay_tools(&self, delay: Duration) {
        self.0.lock().unwrap().tools_delay = delay;
    }

    fn turns_running(&self) -> usize {
        self.0.lock().unwrap().turns.len()
    }

    fn refuse_replies(&self) {
        self.0.lock().unwrap().refuse_replies = true;
    }

    fn compacted(&self) -> Vec<Vec<Compaction>> {
        self.0.lock().unwrap().compacted.clone()
    }

    fn end_compaction(&self, result: Result<Vec<StoredCompaction>, String>) {
        let compaction = self.0.lock().unwrap().compactions.remove(0);
        compaction.send(result).unwrap();
    }

    fn turn(&self) -> Turn {
        let (tx, rx) = oneshot::channel();
        self.0.lock().unwrap().turns.push(tx);
        Box::pin(async move { rx.await.unwrap_or_else(|_| Err("dropped".to_owned())) })
    }
}

impl Host for FakeHost {
    fn start(&self, turn: NewTurn) -> impl Future<Output = Result<(String, Turn), String>> + Send {
        let conversation = {
            let mut log = self.0.lock().unwrap();
            log.conversations += 1;
            log.calls.push(format!(
                "start {:?} titled {:?}, expiring in {}, with {:?}",
                turn.content, turn.title, turn.expires_in, turn.cfg
            ));
            log.cfgs.push(turn.cfg);
            format!("c{}", log.conversations)
        };

        future::ready(Ok((conversation, self.turn())))
    }

    fn query(&self, conversation: &str, content: &str, cfg: Vec<String>) -> Turn {
        {
            let mut log = self.0.lock().unwrap();
            log.calls.push(format!("query {conversation} {content:?}"));
            log.cfgs.push(cfg);
        }
        self.turn()
    }

    fn compact(
        &self,
        conversation: &str,
        compactions: Vec<Compaction>,
    ) -> impl Future<Output = Result<Vec<StoredCompaction>, String>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        {
            let mut log = self.0.lock().unwrap();
            log.calls.push(format!("compact {conversation}"));
            log.compactions.push(tx);
            log.compacted.push(compactions);
        }

        async move { rx.await.unwrap_or_else(|_| Err("dropped".to_owned())) }
    }

    fn reply(
        &self,
        conversation: &str,
        content: &str,
    ) -> impl Future<Output = Result<(), ReplyError>> + Send + 'static {
        let mut log = self.0.lock().unwrap();
        log.calls.push(format!("reply {conversation} {content:?}"));
        let result = if log.refuse_replies {
            Err(ReplyError::Refused("the turn has ended".to_owned()))
        } else {
            Ok(())
        };

        future::ready(result)
    }

    fn stop(
        &self,
        conversation: &str,
    ) -> impl Future<Output = Result<(), String>> + Send + 'static {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(format!("stop {conversation}"));
        future::ready(Ok(()))
    }

    fn tools(&self) -> impl Future<Output = Result<Vec<String>, String>> + Send {
        let (tools, delay) = {
            let log = self.0.lock().unwrap();
            (log.tools.clone(), log.tools_delay)
        };

        async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            tools
        }
    }

    fn events(
        &self,
        conversation: &str,
    ) -> impl Future<Output = Result<Vec<Value>, String>> + Send {
        let answer = self
            .0
            .lock()
            .unwrap()
            .answer
            .clone()
            .unwrap_or_else(|| format!("Answer from {conversation}."));

        future::ready(Ok(vec![
            json!({ "type": "turn_start" }),
            json!({ "type": "chat_response", "message": answer }),
        ]))
    }

    fn log_with(&self, _level: Level, message: &'static str, fields: Map<String, Value>) {
        self.0
            .lock()
            .unwrap()
            .logs
            .push((message.to_owned(), fields));
    }

    fn print(&self, text: &str) {
        self.0.lock().unwrap().prints.push(text.to_owned());
    }
}

#[derive(Default)]
struct FakeOutbox(Mutex<Vec<String>>);

impl FakeOutbox {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

impl Outbox for FakeOutbox {
    fn send(&self, chat: &Chat, markdown: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("send {chat}: {markdown}"));
    }

    fn typing(&self, chat: &Chat, on: bool) {
        let state = if on { "on" } else { "off" };
        self.0
            .lock()
            .unwrap()
            .push(format!("typing {chat} {state}"));
    }
}

#[derive(Clone, Default)]
struct FakeCheckout(Arc<Mutex<CheckoutLog>>);

#[derive(Default)]
struct CheckoutLog {
    calls: Vec<&'static str>,
    stray: Vec<String>,
    dirty: bool,
}

impl FakeCheckout {
    fn calls(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().calls.clone()
    }

    fn write_stray(&self, path: &str) {
        let mut log = self.0.lock().unwrap();
        log.stray = vec![path.to_owned()];
        log.dirty = true;
    }

    fn take(&self) {
        let mut log = self.0.lock().unwrap();
        log.stray.clear();
        log.dirty = false;
    }
}

impl Checkout for FakeCheckout {
    fn begin(&mut self) -> Result<Option<String>, String> {
        self.0.lock().unwrap().calls.push("begin");
        Ok(None)
    }

    fn end(&mut self) -> Result<Vec<String>, String> {
        let mut log = self.0.lock().unwrap();
        log.calls.push("end");
        Ok(log.stray.clone())
    }

    fn stray(&mut self) -> Result<Vec<String>, String> {
        Ok(self.0.lock().unwrap().stray.clone())
    }

    fn idle(&mut self) -> Option<String> {
        None
    }

    fn is_clean(&mut self) -> Result<bool, String> {
        Ok(!self.0.lock().unwrap().dirty)
    }
}

const CLIENT: &str = "+31611111111";
const MAINTAINER: &str = "+31622222222";

fn options() -> Options {
    let Value::Object(options) = json!({
        "persona": "personas/planner",
        "admin": "maintainer",
        "signal": { "account": "+31600000000" },
        "people": {
            "client": { "name": "Ada Client", "signal": CLIENT },
            "maintainer": { "name": "Max Maintainer", "signal": MAINTAINER }
        }
    }) else {
        unreachable!("options are an object");
    };

    Options::read(&options).unwrap()
}

fn november(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
}

struct Fixture {
    bridge: Bridge<FakeHost, FakeCheckout>,
    host: FakeHost,
    outbox: Arc<FakeOutbox>,
    checkout: FakeCheckout,
    today: Arc<Mutex<NaiveDate>>,
}

fn fixture() -> Fixture {
    let host = FakeHost::default();
    let outbox = Arc::new(FakeOutbox::default());
    let checkout = FakeCheckout::default();
    let today = Arc::new(Mutex::new(november(7)));

    let mut bridge = Bridge::new(
        host.clone(),
        Arc::clone(&outbox) as Arc<dyn Outbox>,
        checkout.clone(),
        options(),
        State::in_memory(),
    );
    bridge.conversations = clocked(&today, bridge.options.rollover);

    Fixture {
        bridge,
        host,
        outbox,
        checkout,
        today,
    }
}

fn clocked(today: &Arc<Mutex<NaiveDate>>, rollover: Rollover) -> Conversations {
    let clock = Arc::clone(today);
    Conversations::with_clock(State::in_memory(), rollover, move || *clock.lock().unwrap())
}

fn from(sender: &str, text: &str) -> Inbound {
    Inbound {
        chat: Chat::Direct(sender.to_owned()),
        sender: vec![sender.to_owned()],
        name: None,
        addressed: false,
        leading_mention: false,
        text: text.to_owned(),
        attachments: vec![],
    }
}

const GROUP: &str = "Z3JvdXA=";

fn in_group(sender: &str, text: &str) -> Inbound {
    Inbound {
        chat: Chat::Group(GROUP.to_owned()),
        ..from(sender, text)
    }
}

async fn eventually(condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the condition didn't hold within 5 seconds");
}

impl Fixture {
    fn receive(&mut self, message: Inbound) {
        self.bridge.receive(message);
        self.bridge.advance();
    }

    /// Process the next task event and advance the queue.
    async fn settle(&mut self) {
        let event = tokio::time::timeout(Duration::from_secs(5), self.bridge.events_rx.recv())
            .await
            .expect("no task reported within 5 seconds")
            .expect("the bridge holds a sender");
        self.bridge.handle(event).await;
        self.bridge.advance();
    }

    async fn end_turn(&mut self) {
        self.host.end_turn(Ok(()));
        self.settle().await;
    }
}

#[tokio::test]
async fn the_first_message_starts_a_conversation_and_answers() {
    let mut f = fixture();

    f.receive(from(CLIENT, "What is being worked on?"));
    f.settle().await;
    f.end_turn().await;

    assert_eq!(f.host.calls(), vec![
        "start \"What is being worked on?\" titled \"Chat with Ada Client, 2026-11-07\", expiring \
         in 7d, with [\"personas/planner\", \"user.name=Ada Client\", \
         \"conversation.start_local=true\", \"plugins.command.ticket.options.assistant=Ada Client \
         via jp\", \"plugins.command.rfd.options.assistant=Ada Client via jp\", \
         \"plugins.command.wiki.options.assistant=Ada Client via jp\"]"
    ]);
    assert_eq!(f.outbox.take(), vec![
        "typing direct:+31611111111 on",
        "typing direct:+31611111111 off",
        "send direct:+31611111111: Answer from c1.",
    ]);
    assert_eq!(f.checkout.calls(), vec!["begin", "end"]);
}

#[tokio::test]
async fn later_messages_that_day_continue_the_conversation() {
    let mut f = fixture();
    f.receive(from(CLIENT, "First"));
    f.settle().await;
    f.end_turn().await;

    f.receive(from(CLIENT, "Second"));
    f.settle().await;
    f.end_turn().await;

    *f.today.lock().unwrap() = november(8);
    f.receive(from(CLIENT, "Next day"));
    f.settle().await;
    f.end_turn().await;

    let calls = f.host.calls();
    assert_eq!(calls[1], "query c1 \"Second\"");
    assert!(calls[2].starts_with("start \"Next day\" titled \"Chat with Ada Client, 2026-11-08\""));
}

#[tokio::test]
async fn a_message_during_a_turn_is_handed_to_it() {
    let mut f = fixture();
    f.receive(from(CLIENT, "Rank it first"));
    f.settle().await;

    f.receive(from(CLIENT, "Oops, second"));
    f.settle().await;
    f.end_turn().await;

    assert_eq!(f.host.calls()[1..], ["reply c1 \"Oops, second\""]);
    assert_eq!(
        f.outbox.take().last().unwrap(),
        "send direct:+31611111111: Answer from c1."
    );
}

#[tokio::test]
async fn a_reply_the_turn_refused_starts_the_next_turn() {
    let mut f = fixture();
    f.host.refuse_replies();
    f.receive(from(CLIENT, "Rank it first"));
    f.settle().await;

    f.receive(from(CLIENT, "Oops, second"));
    f.settle().await;
    f.end_turn().await;
    f.settle().await;

    assert_eq!(f.host.calls()[1..], [
        "reply c1 \"Oops, second\"",
        "query c1 \"Oops, second\""
    ]);
}

#[tokio::test]
async fn waiting_messages_go_to_the_next_turn_together() {
    let mut f = fixture();
    f.receive(from(CLIENT, "One"));
    f.receive(from(MAINTAINER, "Also"));
    f.receive(from(MAINTAINER, "this"));
    f.settle().await;
    f.end_turn().await;
    f.settle().await;
    f.end_turn().await;

    assert!(
        f.host.calls()[1].starts_with("start \"Also\\n\\nthis\" titled \"Chat with Max Maintainer")
    );
}

#[tokio::test]
async fn a_message_from_a_stranger_is_dropped() {
    let mut f = fixture();

    f.receive(from("+31699999999", "Hello?"));

    assert!(f.host.calls().is_empty());
    assert!(f.outbox.take().is_empty());
    assert_eq!(f.host.logs(), vec![
        "Dropped message from unconfigured sender."
    ]);
    assert_eq!(
        f.host.logged("Dropped message from unconfigured sender."),
        vec![json!({
            "chat": "direct:+31699999999",
            "sender": ["+31699999999"],
        })]
    );
}

#[tokio::test]
async fn a_group_message_names_its_sender() {
    let mut f = fixture();

    f.receive(Inbound {
        chat: Chat::Group("Z3JvdXA=".to_owned()),
        sender: vec![CLIENT.to_owned()],
        name: Some("Ada".to_owned()),
        addressed: false,
        leading_mention: false,
        text: "Is anything planned for search?".to_owned(),
        attachments: vec![],
    });
    f.settle().await;
    f.end_turn().await;

    assert!(f.host.calls()[0].starts_with(
        "start \"Ada Client: Is anything planned for search?\" titled \"Group chat, 2026-11-07\""
    ));
    // The answer goes to the group, not to the person who asked.
    assert_eq!(f.outbox.take(), vec![
        "typing group:Z3JvdXA= on",
        "typing group:Z3JvdXA= off",
        "send group:Z3JvdXA=: Answer from c1.",
    ]);
}

#[tokio::test]
async fn a_stranger_is_printed_once_with_the_configuration_to_answer_them() {
    let mut f = fixture();
    let stranger = Inbound {
        chat: Chat::Group("Z3JvdXA=".to_owned()),
        sender: vec![
            "33333333-3333-4333-8333-333333333333".to_owned(),
            "+31633333333".to_owned(),
        ],
        name: Some("José O'Brien".to_owned()),
        addressed: false,
        leading_mention: false,
        text: "Hello?".to_owned(),
        attachments: vec![],
    };

    f.receive(stranger.clone());
    f.receive(stranger);
    f.receive(from("+31699999999", "Me too"));

    assert_eq!(f.host.prints(), vec![
        "José O'Brien (33333333-3333-4333-8333-333333333333) wrote in group:Z3JvdXA=, and isn't \
         configured, so the bridge doesn't answer them. To answer them, add this to the bridge's \
         configuration and restart \
         it:\n\n[plugins.command.serve-chat.options.people.jos-o-brien]\nname = \"José \
         O'Brien\"\nsignal = \"33333333-3333-4333-8333-333333333333\"\n",
        "Unknown (+31699999999) wrote in direct:+31699999999, and isn't configured, so the bridge \
         doesn't answer them. To answer them, add this to the bridge's configuration and restart \
         it:\n\n[plugins.command.serve-chat.options.people.unknown]\nname = \"Unknown\"\nsignal = \
         \"+31699999999\"\n",
    ]);
    assert!(f.host.calls().is_empty());
}

#[tokio::test]
async fn an_attachment_reaches_the_turn_as_where_it_was_saved() {
    let mut f = fixture();

    f.receive(Inbound {
        attachments: vec![Attachment {
            content_type: "image/png".to_owned(),
            name: Some("crash.png".to_owned()),
            path: Some("/data/signal-cli/attachments/abc123.png".into()),
        }],
        ..in_group(CLIENT, "Search crashes like this")
    });
    f.settle().await;

    assert!(
        f.host.calls()[0].starts_with(
            "start \"Ada Client: Search crashes like this\\n[Attached crash.png (image/png), \
             saved on the bridge's machine at /data/signal-cli/attachments/abc123.png]\""
        ),
        "{:?}",
        f.host.calls()
    );
}

#[tokio::test]
async fn an_attachment_alone_is_a_message() {
    let mut f = fixture();

    f.receive(Inbound {
        attachments: vec![Attachment {
            content_type: "image/jpeg".to_owned(),
            name: None,
            path: None,
        }],
        ..from(CLIENT, "")
    });
    f.settle().await;

    assert!(
        f.host.calls()[0]
            .starts_with("start \"[Attached image/jpeg, which the bridge couldn't save]\""),
        "{:?}",
        f.host.calls()
    );
}

#[tokio::test]
async fn a_failed_turn_says_so_and_logs_why() {
    let mut f = fixture();
    f.receive(from(CLIENT, "Hi"));
    f.settle().await;

    f.host.end_turn(Err("provider overloaded".to_owned()));
    f.settle().await;

    assert_eq!(
        f.outbox.take().last().unwrap(),
        &format!("send direct:+31611111111: {FAILED}")
    );
    assert_eq!(f.host.logs(), vec!["Turn failed.", "Turn ended."]);
    assert_eq!(f.host.logged("Turn failed."), vec![json!({
        "chat": "direct:+31611111111",
        "error": "provider overloaded",
    })]);
}

#[tokio::test(start_paused = true)]
async fn a_turn_logs_how_long_reading_the_tools_and_running_took() {
    let mut f = fixture();
    f.host.delay_tools(Duration::from_millis(900));

    f.receive(from(CLIENT, "What's next?"));
    f.settle().await;
    tokio::time::advance(Duration::from_millis(5200)).await;
    f.end_turn().await;

    assert_eq!(f.host.logs(), vec!["Turn ended."]);
    assert_eq!(f.host.logged("Turn ended."), vec![json!({
        "chat": "direct:+31611111111",
        "conversation": "c1",
        "total_ms": 6100,
        "checkout_ms": 0,
        "tools_ms": 900,
        "open_ms": 0,
        "run_ms": 5200,
        "answer_ms": 0,
    })]);
}

#[test]
fn the_phases_of_a_turn_add_up_to_its_total() {
    let picked = Instant::now();
    let running = Running {
        chat: Chat::Direct(CLIENT.to_owned()),
        person: "client".to_owned(),
        conversation: Some("c1".to_owned()),
        picked,
        prepared: picked + Duration::from_millis(300),
        accepted: Some((
            picked + Duration::from_millis(1300),
            Duration::from_millis(900),
        )),
    };

    let phases = phases(
        &running,
        picked + Duration::from_millis(6500),
        picked + Duration::from_millis(6700),
    );

    assert_eq!(
        phases.map(Value::Object),
        Some(json!({
            "chat": "direct:+31611111111",
            "conversation": "c1",
            "total_ms": 6700,
            "checkout_ms": 300,
            "tools_ms": 900,
            "open_ms": 100,
            "run_ms": 5200,
            "answer_ms": 200,
        }))
    );
}

#[tokio::test]
async fn a_change_outside_the_writable_directories_pauses_until_taken() {
    let mut f = fixture();
    f.receive(from(CLIENT, "File a bug"));
    f.settle().await;
    f.checkout.write_stray("README.md");
    f.end_turn().await;
    f.outbox.take();

    f.receive(from(CLIENT, "And another"));
    f.receive(from(CLIENT, "And a third"));
    assert_eq!(f.host.calls().len(), 1, "a turn started while paused");
    assert_eq!(f.outbox.take(), vec![format!(
        "send direct:+31611111111: {PAUSED}"
    )]);

    f.checkout.take();
    f.bridge.advance();
    f.settle().await;

    assert_eq!(
        f.host.calls()[1],
        "query c1 \"And another\\n\\nAnd a third\""
    );
    assert_eq!(f.outbox.take(), vec![
        "send direct:+31622222222: The tree is clean again; the bridge has resumed.",
        "typing direct:+31611111111 on",
    ]);
    assert_eq!(f.host.logs(), vec![
        "Turn ended.",
        "Bridge paused.",
        "Bridge resumed."
    ]);
    assert_eq!(f.host.logged("Bridge paused."), vec![json!({
        "stray": ["README.md"],
    })]);
}

#[tokio::test]
async fn the_admin_is_told_what_paused_the_bridge() {
    let mut f = fixture();
    f.receive(from(CLIENT, "File a bug"));
    f.settle().await;
    f.checkout.write_stray("README.md");
    f.end_turn().await;

    assert_eq!(f.outbox.take()[3..], ["send direct:+31622222222: The \
                                       bridge is paused: these changes \
                                       are outside the writable \
                                       directories, so take or discard \
                                       them before it starts another \
                                       turn:\n- README.md"]);
}

/// `options.cfg` entries are applied after `persona` and before the sender
/// attribution overrides.
#[test]
fn extra_cfg_follows_the_persona() {
    let mut f = fixture();
    f.bridge.options.cfg = vec![
        "assistant.model.id=haiku".to_owned(),
        "assistant.model.parameters.reasoning=off".to_owned(),
    ];

    let cfg = cfg(&f.bridge.options, "client");

    assert_eq!(cfg[..3], [
        "personas/planner",
        "assistant.model.id=haiku",
        "assistant.model.parameters.reasoning=off",
    ]);
    assert_eq!(cfg.len(), 8);
}

#[test]
fn the_bridge_sets_one_key_per_cfg_entry() {
    let f = fixture();

    let cfg = cfg(&f.bridge.options, "client");

    for entry in &cfg[1..] {
        assert!(
            !entry.starts_with('{') && entry.contains('='),
            "{entry} isn't a single key"
        );
    }
}

#[tokio::test]
async fn an_addressed_group_message_says_it_is_to_the_assistant() {
    let mut f = fixture();

    f.receive(Inbound {
        chat: Chat::Group("Z3JvdXA=".to_owned()),
        sender: vec![CLIENT.to_owned()],
        name: Some("Ada".to_owned()),
        addressed: true,
        leading_mention: false,
        text: " what's next?".to_owned(),
        attachments: vec![],
    });
    f.settle().await;

    assert!(
        f.host.calls()[0].starts_with("start \"Ada Client (to you): what's next?\""),
        "{:?}",
        f.host.calls()
    );
}

#[tokio::test]
async fn a_silent_answer_sends_nothing() {
    let mut f = fixture();
    f.host.0.lock().unwrap().answer = Some("NO_REPLY.".to_owned());

    f.receive(Inbound {
        chat: Chat::Group("Z3JvdXA=".to_owned()),
        sender: vec![CLIENT.to_owned()],
        name: Some("Ada".to_owned()),
        addressed: false,
        leading_mention: false,
        text: "Lunch at one?".to_owned(),
        attachments: vec![],
    });
    f.settle().await;
    f.end_turn().await;

    assert_eq!(f.outbox.take(), vec![
        "typing group:Z3JvdXA= on",
        "typing group:Z3JvdXA= off",
    ]);
}

#[test]
fn only_the_bare_silent_word_is_silent() {
    for silent in ["NO_REPLY", " NO_REPLY.\n", "`NO_REPLY`", "\"NO_REPLY\""] {
        assert!(is_silent(silent), "{silent:?}");
    }
    for answer in ["No reply needed.", "NO_REPLY, but here is T-1", "no_reply"] {
        assert!(!is_silent(answer), "{answer:?}");
    }
}

#[tokio::test]
async fn a_weekly_rollover_keeps_the_conversation_for_the_week() {
    let mut f = fixture();
    f.bridge.conversations = clocked(&f.today, Rollover::Weekly(Weekday::Sun));

    // 2026-11-07 is a Saturday: its week started on Sunday 2026-11-01.
    f.receive(from(CLIENT, "Saturday"));
    f.settle().await;
    f.end_turn().await;

    *f.today.lock().unwrap() = november(8);
    f.receive(from(CLIENT, "Sunday"));
    f.settle().await;
    f.end_turn().await;

    *f.today.lock().unwrap() = november(14);
    f.receive(from(CLIENT, "Next Saturday"));
    f.settle().await;
    f.end_turn().await;

    let calls = f.host.calls();
    assert!(
        calls[0]
            .starts_with("start \"Saturday\" titled \"Chat with Ada Client, week of 2026-11-01\""),
        "{}",
        calls[0]
    );
    assert!(
        calls[1]
            .starts_with("start \"Sunday\" titled \"Chat with Ada Client, week of 2026-11-08\""),
        "{}",
        calls[1]
    );
    assert_eq!(calls[2], "query c2 \"Next Saturday\"");
}

#[derive(Default)]
struct FakeDecider {
    answers: Mutex<Vec<Result<Verdict, String>>>,
    asked: Mutex<Vec<String>>,
    offered: Mutex<Vec<&'static [Action]>>,
    delay: Duration,
}

impl Decide for FakeDecider {
    fn decide(&self, state: String, actions: &'static [Action]) -> Decision {
        self.asked.lock().unwrap().push(state);
        self.offered.lock().unwrap().push(actions);
        let answer = self.answers.lock().unwrap().remove(0);
        let delay = self.delay;

        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            answer
        })
    }
}

fn command(sender: &str, text: &str) -> Inbound {
    Inbound {
        addressed: true,
        leading_mention: true,
        ..in_group(sender, text)
    }
}

fn verdict(action: Action, probability: f64) -> Verdict {
    Verdict {
        action,
        probability,
    }
}

fn deciding(answers: Vec<Result<Verdict, String>>) -> (Fixture, Arc<FakeDecider>) {
    deciding_after(Duration::ZERO, answers)
}

fn deciding_after(
    delay: Duration,
    answers: Vec<Result<Verdict, String>>,
) -> (Fixture, Arc<FakeDecider>) {
    let mut f = fixture();
    let decider = Arc::new(FakeDecider {
        answers: Mutex::new(answers),
        delay,
        ..FakeDecider::default()
    });
    f.bridge.options.decide = Some(DecideOptions {
        url: "https://decide.invalid".to_owned(),
        model: "jev-latest".to_owned(),
        api_key_env: None,
        threshold: 0.5,
        instructions: "What should the assistant do?".to_owned(),
    });
    f.bridge
        .inbox
        .decide_with(Some(Arc::clone(&decider) as Arc<dyn Decide>));

    (f, decider)
}

#[tokio::test(start_paused = true)]
async fn a_group_message_the_decider_ignores_gets_no_turn() {
    let (mut f, decider) = deciding_after(Duration::from_millis(400), vec![Ok(verdict(
        Action::Ignore,
        0.9,
    ))]);

    f.receive(in_group(CLIENT, "Thanks!"));
    f.settle().await;

    assert!(f.host.calls().is_empty());
    assert_eq!(*decider.asked.lock().unwrap(), vec![
        "Last message:\nAda Client: Thanks!".to_owned()
    ]);
    assert_eq!(f.host.logs(), vec![
        "Message classified.",
        "Message ignored."
    ]);
    assert_eq!(f.host.logged("Message ignored."), vec![json!({
        "chat": "group:Z3JvdXA=",
    })]);
    assert_eq!(f.host.logged("Message classified."), vec![json!({
        "chat": "group:Z3JvdXA=",
        "action": "ignore",
        "probability": 0.9,
        "took_ms": 400,
    })]);
}

#[tokio::test]
async fn a_group_message_the_decider_answers_starts_a_turn() {
    let (mut f, _) = deciding(vec![Ok(verdict(Action::Answer, 0.9))]);

    f.receive(in_group(CLIENT, "What's next?"));
    f.settle().await;
    f.settle().await;

    assert!(
        f.host.calls()[0].starts_with("start \"Ada Client: What's next?\""),
        "{:?}",
        f.host.calls()
    );
}

#[tokio::test]
async fn only_a_message_that_may_give_a_command_is_offered_the_commands() {
    let (mut f, decider) = deciding(vec![
        Ok(verdict(Action::Ignore, 0.9)),
        Ok(verdict(Action::Ignore, 0.9)),
        Ok(verdict(Action::Ignore, 0.9)),
        Ok(verdict(Action::Ignore, 0.9)),
    ]);

    f.receive(command(CLIENT, "compact please"));
    f.settle().await;
    f.receive(Inbound {
        addressed: true,
        ..in_group(CLIENT, "so, compact please?")
    });
    f.settle().await;
    f.receive(in_group(CLIENT, "compact please"));
    f.settle().await;
    f.receive(from(CLIENT, "compact please"));
    f.settle().await;

    assert_eq!(*decider.offered.lock().unwrap(), [
        Action::offered(true),
        Action::offered(false),
        Action::offered(false),
        Action::offered(true),
    ]);
}

#[tokio::test]
async fn a_message_the_decider_ignores_gets_no_turn_even_when_it_names_the_assistant() {
    let (mut f, _) = deciding(vec![Ok(verdict(Action::Ignore, 0.95))]);

    f.receive(command(CLIENT, "thanks!"));
    f.settle().await;

    assert!(f.host.calls().is_empty(), "{:?}", f.host.calls());
}

#[tokio::test(start_paused = true)]
async fn an_unsure_or_failed_decision_answers_the_message() {
    let (mut f, _) = deciding(vec![
        Ok(verdict(Action::Ignore, 0.4)),
        Err("the decide API answered 503".to_owned()),
    ]);

    f.receive(in_group(CLIENT, "Hmm"));
    f.settle().await;
    f.settle().await;
    f.end_turn().await;
    f.receive(in_group(CLIENT, "What's next?"));
    f.settle().await;
    f.settle().await;

    assert_eq!(f.host.calls().len(), 2, "{:?}", f.host.calls());
    assert_eq!(f.host.logs(), vec![
        "Message classified.",
        "Turn ended.",
        "Decide failed, answering.",
    ]);
    assert_eq!(f.host.logged("Decide failed, answering."), vec![json!({
        "chat": "group:Z3JvdXA=",
        "error": "the decide API answered 503",
        "took_ms": 0,
    })]);
}

#[tokio::test]
async fn asking_for_a_new_conversation_starts_the_next_turn_in_one() {
    let (mut f, _) = deciding(vec![
        Ok(verdict(Action::Answer, 0.9)),
        Ok(verdict(Action::NewConversation, 0.9)),
        Ok(verdict(Action::Answer, 0.9)),
    ]);
    f.receive(command(CLIENT, "what's next?"));
    f.settle().await;
    f.settle().await;
    f.end_turn().await;
    f.outbox.take();

    f.receive(command(CLIENT, "start over"));
    f.settle().await;

    assert_eq!(f.outbox.take(), vec![format!(
        "send group:Z3JvdXA=: {NEW_CONVERSATION}"
    )]);
    assert_eq!(f.host.calls().len(), 1, "{:?}", f.host.calls());

    f.receive(command(CLIENT, "what's next?"));
    f.settle().await;
    f.settle().await;

    let calls = f.host.calls();
    assert!(calls[1].starts_with("start "), "{calls:?}");
}

// Messages arriving during compaction stay queued until compaction finishes.
#[tokio::test]
async fn asking_to_compact_compacts_the_conversation_and_says_what_changed() {
    let (mut f, _) = deciding(vec![
        Ok(verdict(Action::Answer, 0.9)),
        Ok(verdict(Action::Compact, 0.9)),
        Ok(verdict(Action::Answer, 0.9)),
    ]);
    f.receive(command(CLIENT, "what's next?"));
    f.settle().await;
    f.settle().await;
    f.end_turn().await;
    f.outbox.take();

    f.receive(command(CLIENT, "please compact"));
    f.settle().await;
    f.receive(command(CLIENT, "and now?"));
    f.settle().await;

    assert_eq!(f.host.calls()[1..], ["compact c1"]);
    assert_eq!(f.host.compacted(), [f.bridge.options.compact.clone()]);

    f.host.end_compaction(Ok(vec![
        StoredCompaction {
            from: 2,
            to: 5,
            reasoning: true,
            tool_calls: Some(ToolCallsMode::Strip),
            summary: None,
        },
        StoredCompaction {
            from: 6,
            to: 6,
            reasoning: false,
            tool_calls: Some(ToolCallsMode::StripResponses),
            summary: None,
        },
        StoredCompaction {
            from: 1,
            to: 4,
            reasoning: true,
            tool_calls: None,
            summary: Some(SummarySource::Generated),
        },
    ]));
    f.settle().await;
    // The waiting message's turn reports `Started` once the host has it.
    f.settle().await;

    assert_eq!(f.outbox.take()[..1], ["send group:Z3JvdXA=: Compacted \
                                       this conversation:\n- turns 2 to \
                                       5: reasoning stripped, tool calls \
                                       stripped\n- turn 6: tool call \
                                       results stripped\n- turns 1 to 4: \
                                       summarized"]);
    assert_eq!(f.host.calls()[2..], [
        "query c1 \"Ada Client (to you): and now?\""
    ]);
}

#[tokio::test]
async fn a_compaction_the_host_refuses_says_why() {
    let (mut f, _) = deciding(vec![
        Ok(verdict(Action::Answer, 0.9)),
        Ok(verdict(Action::Compact, 0.9)),
    ]);
    f.receive(command(CLIENT, "what's next?"));
    f.settle().await;
    f.settle().await;
    f.end_turn().await;
    f.outbox.take();

    f.receive(command(CLIENT, "please compact"));
    f.settle().await;
    f.host
        .end_compaction(Err("No turns to compact in conversation c1.".to_owned()));
    f.settle().await;

    assert_eq!(f.outbox.take(), ["send group:Z3JvdXA=: I couldn't \
                                  compact this conversation: No turns to \
                                  compact in conversation c1."]);
}

#[tokio::test]
async fn asking_to_compact_during_a_turn_in_that_chat_waits_for_the_answer() {
    let (mut f, _) = deciding(vec![
        Ok(verdict(Action::Answer, 0.9)),
        Ok(verdict(Action::Answer, 0.9)),
        Ok(verdict(Action::Compact, 0.9)),
    ]);
    f.receive(command(CLIENT, "what's next?"));
    f.settle().await;
    f.settle().await;
    f.end_turn().await;
    f.receive(command(CLIENT, "and after that?"));
    f.settle().await;
    f.settle().await;
    f.outbox.take();

    f.receive(command(CLIENT, "please compact"));
    f.settle().await;

    assert_eq!(f.outbox.take(), vec![format!(
        "send group:Z3JvdXA=: {BUSY_COMPACTING}"
    )]);
    assert!(
        f.host
            .calls()
            .iter()
            .all(|call| !call.starts_with("compact")),
        "{:?}",
        f.host.calls()
    );
}

#[tokio::test]
async fn asking_to_compact_before_any_conversation_says_there_is_none() {
    let (mut f, _) = deciding(vec![Ok(verdict(Action::Compact, 0.9))]);

    f.receive(from(CLIENT, "compact please"));
    f.settle().await;

    assert_eq!(f.outbox.take(), vec![format!(
        "send direct:+31611111111: {NOTHING_TO_COMPACT}"
    )]);
    assert!(f.host.calls().is_empty());
}

#[tokio::test]
async fn a_correction_the_decider_answers_goes_into_the_running_turn() {
    let (mut f, _) = deciding(vec![
        Ok(verdict(Action::Answer, 0.9)),
        Ok(verdict(Action::Answer, 0.9)),
    ]);
    f.receive(command(CLIENT, "rank T-1 first"));
    f.settle().await;
    f.settle().await;

    f.receive(in_group(CLIENT, "actually second"));
    f.settle().await;
    f.settle().await;

    assert_eq!(f.host.calls()[1..], [
        "reply c1 \"Ada Client: actually second\""
    ]);
}

#[tokio::test]
async fn a_decision_reads_the_assistants_last_answer() {
    let (mut f, decider) = deciding(vec![
        Ok(verdict(Action::Answer, 0.9)),
        Ok(verdict(Action::Answer, 0.9)),
    ]);
    f.receive(command(CLIENT, "what's next?"));
    f.settle().await;
    f.settle().await;
    f.end_turn().await;

    f.receive(in_group(CLIENT, "why?"));
    f.settle().await;

    assert_eq!(
        decider.asked.lock().unwrap()[1],
        "Earlier messages, oldest first:\nAda Client (to you): what's next?\nAssistant: Answer \
         from c1.\n\nLast message:\nAda Client: why?"
    );
}

#[tokio::test]
async fn corrections_reach_the_turn_in_the_order_they_were_sent() {
    let mut f = fixture();
    f.receive(from(CLIENT, "Move T to the top"));
    f.receive(from(CLIENT, "Actually second"));
    f.settle().await;

    f.receive(from(CLIENT, "Actually third"));
    f.settle().await;
    f.settle().await;
    f.end_turn().await;

    assert_eq!(f.host.calls()[1..], [
        "reply c1 \"Actually second\"",
        "reply c1 \"Actually third\"",
    ]);
}

#[tokio::test]
async fn a_refused_reply_goes_ahead_of_what_came_after_it() {
    let mut f = fixture();
    f.host.refuse_replies();
    f.receive(from(CLIENT, "Move T to the top"));
    f.settle().await;

    f.receive(from(CLIENT, "Actually second"));
    f.receive(from(CLIENT, "Actually third"));
    f.settle().await;
    f.end_turn().await;
    f.settle().await;

    assert_eq!(f.host.calls()[1..], [
        "reply c1 \"Actually second\"",
        "query c1 \"Actually second\\n\\nActually third\"",
    ]);
}

#[tokio::test]
async fn another_persons_message_gets_a_turn_of_its_own() {
    let mut f = fixture();
    f.receive(in_group(CLIENT, "File a bug about search"));
    f.settle().await;

    f.receive(in_group(MAINTAINER, "And one about the board"));
    f.receive(in_group(CLIENT, "Make it high priority"));
    f.end_turn().await;
    f.settle().await;
    f.end_turn().await;
    f.settle().await;

    assert_eq!(f.host.calls()[1..], [
        "query c1 \"Max Maintainer: And one about the board\"",
        "query c1 \"Ada Client: Make it high priority\"",
    ]);
    let names: Vec<String> = f
        .host
        .cfgs()
        .iter()
        .filter_map(|cfg| {
            cfg.iter()
                .find(|entry| entry.starts_with("user.name="))
                .cloned()
        })
        .collect();
    assert_eq!(names, [
        "user.name=Ada Client",
        "user.name=Max Maintainer",
        "user.name=Ada Client"
    ]);
}

#[test]
fn a_turn_disables_every_configured_tool_before_the_persona() {
    let cfg = turn_cfg(&["bash".to_owned(), "ticket_list".to_owned()], vec![
        "personas/planner".to_owned(),
    ])
    .unwrap();

    assert_eq!(cfg.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&cfg[0]).unwrap(),
        json!({ "conversation": { "tools": {
            "bash": { "enable": false },
            "ticket_list": { "enable": false },
        } } })
    );
    assert_eq!(cfg[1], "personas/planner");
}

#[test]
fn a_tool_no_cfg_key_can_name_refuses_the_turn() {
    assert_eq!(
        turn_cfg(&["search.web".to_owned()], vec![]),
        Err(
            "the tool `search.web` can't be disabled, because a `.` in its name splits a `--cfg` \
             key"
            .to_owned()
        )
    );
}

#[tokio::test]
async fn every_turn_disables_the_configured_tools() {
    let mut f = fixture();
    f.host.set_tools(Ok(vec!["bash".to_owned()]));

    f.receive(from(CLIENT, "First"));
    f.settle().await;
    f.end_turn().await;
    f.receive(from(CLIENT, "Second"));
    f.settle().await;
    f.end_turn().await;

    let cfgs = f.host.cfgs();
    assert_eq!(cfgs.len(), 2);
    for cfg in cfgs {
        assert_eq!(
            serde_json::from_str::<Value>(&cfg[0]).unwrap(),
            json!({ "conversation": { "tools": { "bash": { "enable": false } } } })
        );
        assert_eq!(cfg[1], "personas/planner");
    }
}

#[tokio::test]
async fn a_turn_whose_tools_cant_be_read_does_not_start() {
    let mut f = fixture();
    f.host
        .set_tools(Err("config path not found: conversation".to_owned()));

    f.receive(from(CLIENT, "Hi"));
    f.settle().await;

    assert!(f.host.calls().is_empty());
    assert_eq!(f.outbox.take(), vec![
        "typing direct:+31611111111 on".to_owned(),
        "typing direct:+31611111111 off".to_owned(),
        format!("send direct:+31611111111: {FAILED}"),
    ]);
    assert_eq!(f.host.logged("Turn failed."), vec![json!({
        "chat": "direct:+31611111111",
        "error": "couldn't read the configured tools: config path not found: conversation",
    })]);
    assert_eq!(f.host.logs(), vec!["Turn failed."]);
    assert_eq!(f.checkout.calls(), vec!["begin", "end"]);
}

#[test]
fn tool_names_skip_the_defaults_and_unwrap_a_strategy() {
    assert_eq!(
        tool_names(Some(
            &json!({ "*": { "run": "ask" }, "bash": {}, "fs_read_file": {} })
        )),
        Ok(vec!["bash".to_owned(), "fs_read_file".to_owned()])
    );
    assert_eq!(
        tool_names(Some(
            &json!({ "value": { "bash": {} }, "strategy": "replace" })
        )),
        Ok(vec!["bash".to_owned()])
    );
    assert_eq!(tool_names(None), Ok(vec![]));
    assert_eq!(
        tool_names(Some(&json!("bash"))),
        Err("`conversation.tools` is not a table: \"bash\"".to_owned())
    );
}

#[tokio::test]
async fn the_transport_stopping_waits_for_the_running_turn() {
    let Fixture {
        bridge,
        host,
        checkout,
        ..
    } = fixture();
    let (inbound_tx, inbound_rx) = mpsc::channel(8);
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    inbound_tx.send(from(CLIENT, "File a bug")).await.unwrap();
    drop(inbound_tx);

    let run = tokio::spawn(bridge.run(inbound_rx, shutdown_rx));
    eventually(|| host.turns_running() == 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        !run.is_finished(),
        "the bridge exited with its turn running"
    );
    assert_eq!(checkout.calls(), vec!["begin"]);

    host.end_turn(Ok(()));
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result, Err("The chat transport stopped.".to_owned()));
    assert_eq!(checkout.calls(), vec!["begin", "end"]);
    assert!(!host.calls().iter().any(|call| call.starts_with("stop")));
}

#[tokio::test]
async fn a_shutdown_stops_the_running_turn_and_waits_for_it() {
    let Fixture {
        bridge,
        host,
        checkout,
        ..
    } = fixture();
    let (inbound_tx, inbound_rx) = mpsc::channel(8);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    inbound_tx.send(from(CLIENT, "File a bug")).await.unwrap();

    let run = tokio::spawn(bridge.run(inbound_rx, shutdown_rx));
    eventually(|| host.turns_running() == 1).await;
    shutdown_tx.send(true).unwrap();
    eventually(|| host.calls().contains(&"stop c1".to_owned())).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        !run.is_finished(),
        "the bridge exited with its turn running"
    );
    assert_eq!(checkout.calls(), vec!["begin"]);

    host.end_turn(Ok(()));
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result, Ok(()));
    assert_eq!(checkout.calls(), vec!["begin", "end"]);
    drop(inbound_tx);
}

#[test]
fn the_planner_persona_enables_only_its_own_tools_over_a_permissive_base() {
    let root = Utf8PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../../..");
    let mut base = load_partial_at_path(root.join(".jp/config.toml").as_std_path())
        .unwrap()
        .unwrap();
    for key in [
        "conversation.tools.fs_modify_file.enable=true",
        "conversation.tools.bash.enable=true",
    ] {
        base.assign(key.parse::<KvAssignment>().unwrap()).unwrap();
    }

    let tools = tool_names(
        serde_json::to_value(&base)
            .unwrap()
            .pointer("/conversation/tools"),
    )
    .unwrap();
    let resolve = |cfg: Vec<String>| -> AppConfig {
        let mut partial = base.clone();
        for entry in cfg {
            if entry.starts_with('{') {
                let value = serde_json::from_str(&entry).unwrap();
                partial.assign(KvAssignment::root_json(value)).unwrap();
            } else if entry.contains('=') {
                partial
                    .assign(entry.parse::<KvAssignment>().unwrap())
                    .unwrap();
            } else {
                let file = load_partial_at_path(root.join(".jp/config").join(&entry).as_std_path())
                    .unwrap()
                    .unwrap();
                partial.merge(&(), file).unwrap();
            }
        }
        build(partial).unwrap()
    };
    let enabled = |config: &AppConfig| -> BTreeSet<String> {
        tools
            .iter()
            .filter(|name| {
                config
                    .conversation
                    .tools
                    .get(name)
                    .is_some_and(|tool| tool.is_enabled())
            })
            .cloned()
            .collect()
    };

    let f = fixture();
    let cfg = cfg(&f.bridge.options, "client");

    // The persona alone doesn't override a tool another layer enables.
    let unguarded = enabled(&resolve(cfg.clone()));
    assert!(
        unguarded.contains("bash") && unguarded.contains("fs_modify_file"),
        "{unguarded:?}"
    );

    let guarded = enabled(&resolve(turn_cfg(&tools, cfg).unwrap()));
    assert_eq!(
        guarded,
        BTreeSet::from(
            [
                "fs_grep_files",
                "fs_grep_user_docs",
                "fs_list_files",
                "fs_read_file",
                "rfd_abandon",
                "rfd_draft",
                "rfd_extend",
                "rfd_list",
                "rfd_promote",
                "rfd_rename",
                "rfd_renumber",
                "rfd_require",
                "rfd_show",
                "rfd_supersede",
                "ticket_comment",
                "ticket_create",
                "ticket_edit",
                "ticket_list",
                "ticket_move",
                "ticket_show",
                "wiki_create",
                "wiki_edit",
                "wiki_list",
                "wiki_show",
            ]
            .map(ToOwned::to_owned)
        )
    );
}
