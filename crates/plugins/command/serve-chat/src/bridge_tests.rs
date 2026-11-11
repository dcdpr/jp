use std::{
    future::{self, Future},
    sync::Mutex,
};

use chrono::Weekday;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::*;
use crate::host::Turn;

#[derive(Default)]
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
            format!("c{}", log.conversations)
        };

        future::ready(Ok((conversation, self.turn())))
    }

    fn query(&self, conversation: &str, content: &str, _cfg: Vec<String>) -> Turn {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(format!("query {conversation} {content:?}"));
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
