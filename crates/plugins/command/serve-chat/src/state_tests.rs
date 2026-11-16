use std::sync::{Arc, Mutex};

use camino_tempfile::Utf8TempDir;
use chrono::Weekday;

use super::*;

fn day(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
}

#[test]
fn a_conversation_holds_for_the_day_it_was_started() {
    let mut state = State::in_memory();
    let chat = Chat::Direct("+31600000000".to_owned());

    state.set(&chat, day(7), "17624160000").unwrap();

    assert_eq!(state.conversation(&chat, day(7)), Some("17624160000"));
    assert_eq!(state.conversation(&chat, day(8)), None);
    assert_eq!(
        state.conversation(&Chat::Group("+31600000000".to_owned()), day(7)),
        None
    );
}

#[test]
fn the_state_survives_a_restart() {
    let dir = Utf8TempDir::new().unwrap();
    let chat = Chat::Group("Z3JvdXA=".to_owned());

    State::open(dir.path())
        .unwrap()
        .set(&chat, day(7), "17624160000")
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join(STATE_FILE)).unwrap(),
        "{\n  \"chats\": {\n    \"group:Z3JvdXA=\": {\n      \"date\": \"2026-11-07\",\n      \
         \"conversation\": \"17624160000\"\n    }\n  }\n}\n"
    );
    assert_eq!(
        State::open(dir.path()).unwrap().conversation(&chat, day(7)),
        Some("17624160000")
    );
}

/// Forgotten on disk too, so a restart doesn't bring the old conversation back.
#[test]
fn a_forgotten_chat_has_no_conversation_after_a_restart() {
    let dir = Utf8TempDir::new().unwrap();
    let chat = Chat::Group("Z3JvdXA=".to_owned());
    let other = Chat::Direct("+31600000000".to_owned());
    let mut state = State::open(dir.path()).unwrap();
    state.set(&chat, day(7), "17624160000").unwrap();
    state.set(&other, day(7), "17624160001").unwrap();

    state.forget(&chat).unwrap();

    let reopened = State::open(dir.path()).unwrap();
    assert_eq!(reopened.conversation(&chat, day(7)), None);
    assert_eq!(reopened.conversation(&other, day(7)), Some("17624160001"));
}

#[test]
fn the_current_conversation_ends_with_its_week() {
    let today = Arc::new(Mutex::new(day(7)));
    let clock = Arc::clone(&today);
    let mut conversations = Conversations::with_clock(
        State::in_memory(),
        Rollover::Weekly(Weekday::Sun),
        move || *clock.lock().unwrap(),
    );
    let chat = Chat::Direct("+31600000000".to_owned());

    // 2026-11-07 is a Saturday, in the week that started on Sunday 2026-11-01.
    conversations.record(&chat, "17624160000").unwrap();
    assert_eq!(conversations.period(), day(1));
    assert_eq!(conversations.current(&chat), Some("17624160000"));

    *today.lock().unwrap() = day(8);
    assert_eq!(conversations.period(), day(8));
    assert_eq!(conversations.current(&chat), None);
}

/// Every turn records its conversation, so recording the current one again must
/// not rewrite the file.
#[test]
fn recording_the_current_conversation_writes_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(STATE_FILE);
    let mut conversations =
        Conversations::with_clock(State::open(dir.path()).unwrap(), Rollover::Daily, || day(7));
    let chat = Chat::Direct("+31600000000".to_owned());

    conversations.record(&chat, "17624160000").unwrap();
    fs::remove_file(&path).unwrap();
    conversations.record(&chat, "17624160000").unwrap();
    assert!(
        !path.exists(),
        "the unchanged conversation was written again"
    );

    conversations.record(&chat, "17624160001").unwrap();
    assert!(path.exists(), "a new conversation wasn't written");
}

#[test]
fn an_unreadable_state_file_is_an_error() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join(STATE_FILE), "not json").unwrap();

    let error = State::open(dir.path()).unwrap_err();

    assert!(
        error.ends_with("is not the bridge's state: expected ident at line 1 column 2"),
        "{error}"
    );
}
