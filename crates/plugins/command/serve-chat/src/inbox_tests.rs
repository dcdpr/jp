use std::sync::Mutex;

use pretty_assertions::assert_eq;

use super::*;
use crate::decide::{Action, Verdict};

/// Records what it was asked, and answers every question with "answer".
#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Decide for Recorder {
    fn decide(&self, state: String, _actions: &'static [Action]) -> Decision {
        self.0.lock().unwrap().push(state);
        Box::pin(async {
            Ok(Verdict {
                action: Action::Answer,
                probability: 1.0,
            })
        })
    }
}

fn group() -> Chat {
    Chat::Group("Z3JvdXA=".to_owned())
}

fn incoming(text: &str, commands: bool, context: Option<&str>) -> Incoming {
    Incoming {
        chat: group(),
        person: "ada".to_owned(),
        text: text.to_owned(),
        commands,
        context: context.map(ToOwned::to_owned),
    }
}

fn inbox() -> (Inbox, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let mut inbox = Inbox::default();
    inbox.decide_with(Some(Arc::clone(&recorder) as Arc<dyn Decide>));
    (inbox, recorder)
}

fn take(inbox: &mut Inbox) -> Option<Incoming> {
    match inbox.next()? {
        Next::Take(incoming) => Some(incoming),
        Next::Decide(_) => panic!("expected a message to take, got a decision"),
    }
}

#[test]
fn without_a_decider_a_message_is_taken_at_once() {
    let mut inbox = Inbox::default();
    inbox.push(
        group(),
        "ada".to_owned(),
        "Ada Client: thanks!".to_owned(),
        false,
    );

    assert_eq!(
        take(&mut inbox),
        Some(incoming("Ada Client: thanks!", false, None))
    );
}

#[test]
fn a_message_waits_behind_one_being_decided() {
    let (mut inbox, recorder) = inbox();
    inbox.push(
        group(),
        "ada".to_owned(),
        "Ada Client: rank it first".to_owned(),
        false,
    );
    inbox.push(
        group(),
        "ada".to_owned(),
        "Ada Client (to you): actually second".to_owned(),
        true,
    );

    assert!(matches!(inbox.next(), Some(Next::Decide(_))));
    assert!(inbox.next().is_none(), "a message overtook the decision");
    assert_eq!(recorder.0.lock().unwrap().len(), 1);

    assert_eq!(
        inbox.decided(),
        Some(incoming(
            "Ada Client: rank it first",
            false,
            Some("Last message:\nAda Client: rank it first")
        ))
    );
    assert!(matches!(inbox.next(), Some(Next::Decide(_))));
    assert_eq!(recorder.0.lock().unwrap().len(), 2);
}

#[test]
fn a_decision_reads_the_lines_before_its_message() {
    let (mut inbox, recorder) = inbox();
    inbox.remember(&group(), "Assistant: Filed T-1 for review.");
    inbox.push(
        group(),
        "ada".to_owned(),
        "Ada Client: thanks!".to_owned(),
        false,
    );
    inbox.push(
        group(),
        "max".to_owned(),
        "Max Maintainer: lunch?".to_owned(),
        false,
    );

    drop(inbox.next());

    assert_eq!(*recorder.0.lock().unwrap(), vec![
        "Earlier messages, oldest first:\nAssistant: Filed T-1 for review.\n\nLast message:\nAda \
         Client: thanks!"
            .to_owned()
    ]);
}

#[test]
fn a_chat_keeps_only_its_last_lines() {
    let (mut inbox, recorder) = inbox();
    for index in 0..10 {
        inbox.remember(&group(), &format!("Ada Client: {index}"));
    }
    inbox.push(
        group(),
        "ada".to_owned(),
        "Ada Client: last".to_owned(),
        false,
    );

    drop(inbox.next());

    let context = recorder.0.lock().unwrap()[0].clone();
    assert!(!context.contains("Ada Client: 1\n"), "{context}");
    assert!(context.contains("Ada Client: 2\n"), "{context}");
}

#[test]
fn a_stranger_is_seen_once() {
    let mut inbox = Inbox::default();

    assert!(inbox.first_sighting("+31699999999"));
    assert!(!inbox.first_sighting("+31699999999"));
}
