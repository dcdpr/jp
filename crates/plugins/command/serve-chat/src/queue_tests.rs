use pretty_assertions::assert_eq;

use super::*;

fn direct(account: &str) -> Chat {
    Chat::Direct(account.to_owned())
}

fn group() -> Chat {
    Chat::Group("Z3JvdXA=".to_owned())
}

fn waiting(chat: Chat, person: &str, texts: &[&str]) -> Waiting {
    Waiting {
        chat,
        person: person.to_owned(),
        texts: texts.iter().map(|text| (*text).to_owned()).collect(),
    }
}

#[test]
fn consecutive_messages_from_one_person_form_one_batch() {
    let mut queue = Queue::default();
    queue.push(direct("a"), "ada".to_owned(), "One".to_owned());
    queue.push(direct("a"), "ada".to_owned(), "Two".to_owned());

    assert_eq!(
        queue.next_turn(),
        Some(waiting(direct("a"), "ada", &["One", "Two"]))
    );
    assert_eq!(queue.next_turn(), None);
}

#[test]
fn a_held_queue_starts_no_turn_until_released() {
    let mut queue = Queue::default();
    queue.hold();
    queue.push(direct("a"), "ada".to_owned(), "One".to_owned());

    assert_eq!(queue.next_turn(), None);

    queue.release();

    assert_eq!(
        queue.next_turn(),
        Some(waiting(direct("a"), "ada", &["One"]))
    );
}

#[test]
fn a_queue_held_twice_waits_for_both_releases() {
    let mut queue = Queue::default();
    queue.hold();
    queue.hold();
    queue.push(direct("a"), "ada".to_owned(), "One".to_owned());

    queue.release();
    assert_eq!(queue.next_turn(), None);

    queue.release();
    assert_eq!(
        queue.next_turn(),
        Some(waiting(direct("a"), "ada", &["One"]))
    );
}

#[test]
fn another_person_starts_a_batch_of_their_own() {
    let mut queue = Queue::default();
    queue.push(group(), "ada".to_owned(), "One".to_owned());
    queue.push(group(), "max".to_owned(), "Two".to_owned());
    queue.push(group(), "ada".to_owned(), "Three".to_owned());

    assert_eq!(queue.next_turn(), Some(waiting(group(), "ada", &["One"])));
    assert_eq!(queue.next_turn(), Some(waiting(group(), "max", &["Two"])));
    assert_eq!(queue.next_turn(), Some(waiting(group(), "ada", &["Three"])));
}

#[test]
fn a_reply_is_claimed_once_until_it_is_answered() {
    let mut queue = Queue::default();

    assert!(queue.try_reply(&direct("a")));
    assert!(!queue.try_reply(&direct("a")));

    queue.replied();
    assert!(queue.try_reply(&direct("a")));
}

#[test]
fn a_reply_waits_behind_an_earlier_message_from_its_chat() {
    let mut queue = Queue::default();
    queue.push(direct("a"), "ada".to_owned(), "Second".to_owned());

    assert!(!queue.try_reply(&direct("a")));
    assert!(queue.try_reply(&direct("b")));
}

#[test]
fn nothing_starts_a_turn_while_a_reply_is_in_flight() {
    let mut queue = Queue::default();
    queue.push(direct("b"), "max".to_owned(), "Hi".to_owned());
    assert!(queue.try_reply(&direct("a")));

    assert_eq!(queue.next_turn(), None);

    queue.replied();
    assert_eq!(
        queue.next_turn(),
        Some(waiting(direct("b"), "max", &["Hi"]))
    );
}

#[test]
fn the_next_reply_is_the_first_batch_for_the_chat_from_the_turns_person() {
    let mut queue = Queue::default();
    queue.push(group(), "max".to_owned(), "Mine".to_owned());
    queue.push(direct("a"), "ada".to_owned(), "Elsewhere".to_owned());

    assert_eq!(queue.next_reply(&group(), "ada"), None);
    assert_eq!(
        queue.next_reply(&group(), "max"),
        Some(waiting(group(), "max", &["Mine"]))
    );
    // Taking it put a reply in flight.
    assert_eq!(queue.next_reply(&direct("a"), "ada"), None);
    assert_eq!(queue.next_turn(), None);
}

#[test]
fn a_refused_reply_goes_ahead_of_its_chat() {
    let mut queue = Queue::default();
    queue.push(direct("b"), "max".to_owned(), "Other chat".to_owned());
    queue.push(direct("a"), "ada".to_owned(), "Third".to_owned());

    queue.push_first(direct("a"), "ada".to_owned(), "Second".to_owned());

    assert_eq!(
        queue.next_turn(),
        Some(waiting(direct("b"), "max", &["Other chat"]))
    );
    assert_eq!(
        queue.next_turn(),
        Some(waiting(direct("a"), "ada", &["Second", "Third"]))
    );
}

#[test]
fn a_refused_reply_from_another_person_gets_a_batch_of_its_own() {
    let mut queue = Queue::default();
    queue.push(group(), "max".to_owned(), "Later".to_owned());

    queue.push_first(group(), "ada".to_owned(), "Earlier".to_owned());

    assert_eq!(
        queue.next_turn(),
        Some(waiting(group(), "ada", &["Earlier"]))
    );
    assert_eq!(queue.next_turn(), Some(waiting(group(), "max", &["Later"])));
}

#[test]
fn a_restored_batch_runs_next() {
    let mut queue = Queue::default();
    queue.push(direct("b"), "max".to_owned(), "Later".to_owned());

    queue.restore(waiting(direct("a"), "ada", &["First"]));

    assert_eq!(
        queue.next_turn(),
        Some(waiting(direct("a"), "ada", &["First"]))
    );
}

#[test]
fn a_closed_queue_releases_nothing() {
    let mut queue = Queue::default();
    queue.push(direct("a"), "ada".to_owned(), "Hi".to_owned());

    queue.close();

    assert_eq!(queue.next_turn(), None);
    assert_eq!(queue.next_reply(&direct("a"), "ada"), None);
    assert!(!queue.try_reply(&direct("b")));
}
