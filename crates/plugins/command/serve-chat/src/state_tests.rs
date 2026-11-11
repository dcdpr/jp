use camino_tempfile::Utf8TempDir;

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

/// The mark survives a restart, and is taken once.
#[test]
fn a_compaction_mark_is_kept_until_it_is_taken() {
    let dir = Utf8TempDir::new().unwrap();
    let chat = Chat::Group("Z3JvdXA=".to_owned());
    let mut state = State::open(dir.path()).unwrap();
    state.set(&chat, day(7), "17624160000").unwrap();

    assert_eq!(state.compact_next(&chat, day(7)), Ok(true));

    let mut reopened = State::open(dir.path()).unwrap();
    assert_eq!(reopened.take_compact(&chat), Ok(true));
    assert_eq!(reopened.take_compact(&chat), Ok(false));
    assert_eq!(
        State::open(dir.path()).unwrap().take_compact(&chat),
        Ok(false)
    );
}

/// Yesterday's conversation is not the one a daily chat's next turn uses.
#[test]
fn a_chat_without_a_conversation_this_period_has_nothing_to_compact() {
    let mut state = State::in_memory();
    let chat = Chat::Group("Z3JvdXA=".to_owned());
    state.set(&chat, day(6), "17624160000").unwrap();

    assert_eq!(state.compact_next(&chat, day(7)), Ok(false));
    assert_eq!(
        state.compact_next(&Chat::Direct("+31600000000".to_owned()), day(7)),
        Ok(false)
    );
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
