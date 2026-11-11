use std::{
    collections::BTreeSet,
    future::{self, Future},
    sync::Mutex,
};

use camino::Utf8PathBuf;
use chrono::Weekday;
use jp_config::{
    AppConfig, PartialConfig as _,
    assignment::{AssignKeyValue as _, KvAssignment},
    util::{build, load_partial_at_path},
};
use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

use super::*;
use crate::host::{Turn, tool_names};

struct HostLog {
    calls: Vec<String>,
    turns: Vec<oneshot::Sender<Result<(), String>>>,
    conversations: u32,
    refuse_replies: bool,
    logs: Vec<String>,
    prints: Vec<String>,

    /// Custom response text returned instead of `"Answer from
    /// <conversation>."`.
    answer: Option<String>,

    /// The `--cfg` of every turn started, in order.
    cfgs: Vec<Vec<String>>,

    /// What [`Host::tools`] answers.
    tools: Result<Vec<String>, String>,
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
        self.0.lock().unwrap().logs.clone()
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

    fn turns_running(&self) -> usize {
        self.0.lock().unwrap().turns.len()
    }

    fn refuse_replies(&self) {
        self.0.lock().unwrap().refuse_replies = true;
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
        future::ready(self.0.lock().unwrap().tools.clone())
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

    fn log(&self, _level: Level, message: &str) {
        self.0.lock().unwrap().logs.push(message.to_owned());
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

    let clock = Arc::clone(&today);
    let bridge = Bridge::new(
        host.clone(),
        Arc::clone(&outbox) as Arc<dyn Outbox>,
        checkout.clone(),
        options(),
        State::in_memory(),
    )
    .with_today(move || *clock.lock().unwrap());

    Fixture {
        bridge,
        host,
        outbox,
        checkout,
        today,
    }
}

fn from(sender: &str, text: &str) -> Inbound {
    Inbound {
        chat: Chat::Direct(sender.to_owned()),
        sender: vec![sender.to_owned()],
        name: None,
        addressed: false,
        text: text.to_owned(),
        attachments: 0,
    }
}

const GROUP: &str = "Z3JvdXA=";

fn in_group(sender: &str, text: &str) -> Inbound {
    Inbound {
        chat: Chat::Group(GROUP.to_owned()),
        ..from(sender, text)
    }
}

/// Wait until `condition` holds, failing after five seconds.
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
        let event = tokio::time::timeout(Duration::from_secs(5), self.bridge.next_event())
            .await
            .expect("no task reported within 5 seconds");
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
        "Dropped a message from +31699999999 in direct:+31699999999: not one of the configured \
         people."
    ]);
}

#[tokio::test]
async fn a_group_message_names_its_sender() {
    let mut f = fixture();

    f.receive(Inbound {
        chat: Chat::Group("Z3JvdXA=".to_owned()),
        sender: vec![CLIENT.to_owned()],
        name: Some("Ada".to_owned()),
        addressed: false,
        text: "Is anything planned for search?".to_owned(),
        attachments: 0,
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
        text: "Hello?".to_owned(),
        attachments: 0,
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
async fn an_attachment_is_answered_with_what_the_bridge_can_read() {
    let mut f = fixture();

    f.receive(Inbound {
        attachments: 1,
        ..from(CLIENT, "")
    });

    assert!(f.host.calls().is_empty());
    assert_eq!(f.outbox.take(), vec![format!(
        "send direct:+31611111111: {TEXT_ONLY}"
    )]);
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
    assert_eq!(f.host.logs(), vec![
        "A turn for direct:+31611111111 failed: provider overloaded"
    ]);
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

    let cfg = f.bridge.cfg("client");

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

    let cfg = f.bridge.cfg("client");

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
        text: " what's next?".to_owned(),
        attachments: 0,
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
        text: "Lunch at one?".to_owned(),
        attachments: 0,
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
    f.bridge.options.rollover = Rollover::Weekly(Weekday::Sun);

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

/// "Move it to the top", then "actually second" while the conversation is still
/// being created, then "actually third" once it exists: the turn gets the
/// corrections in the order they were sent, one reply at a time.
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

/// A reply the turn didn't take goes to the next turn ahead of the message that
/// arrived while it was in flight.
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

/// A group message from someone other than the person a turn runs as waits for
/// a turn of its own, so what it asks for is attributed to them; the next
/// message from the first person waits behind it.
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

/// Without the tool list there is nothing to disable, and a turn run anyway
/// would have whatever tools the base configuration enables.
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
    assert_eq!(f.host.logs(), vec![
        "A turn for direct:+31611111111 failed: couldn't read the configured tools: config path \
         not found: conversation"
    ]);
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

/// The transport stopping mid-turn doesn't end the turn: the host runs it to
/// its end after the plugin exits.
/// The bridge keeps the checkout locked until then, and checks what the turn
/// wrote.
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

/// A shutdown asks the running turn to stop, and keeps the checkout locked
/// until it has.
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

/// The workspace's planner persona, under the bridge's per-turn settings and
/// layered the way the host layers `--cfg`, leaves enabled exactly the tools
/// the persona names, over a configuration that enables a file-writing tool and
/// a shell.
///
/// `--cfg` entries are applied as `jp` reads them: a JSON object at the root,
/// `key=value` as one assignment, and anything else as a file under
/// `.jp/config`.
#[test]
fn the_planner_persona_enables_only_its_own_tools_over_a_permissive_base() {
    let root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..");
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
    let cfg = f.bridge.cfg("client");

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
