use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::{Utf8TempDir, tempdir};
use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;
use test_log::test;

use super::*;
use crate::{CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE};

fn now() -> DateTime<Utc> {
    "2026-11-19T12:00:00Z".parse().unwrap()
}

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

/// A storage root with no user-local storage, and its `conversations/`.
fn single_root() -> (Utf8TempDir, Storage, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("ws")).unwrap();
    let conversations = tmp.path().join("ws").join(CONVERSATIONS_DIR);
    fs::create_dir_all(&conversations).unwrap();
    (tmp, storage, conversations)
}

fn write_dir(conversations: &Utf8Path, dirname: &str, files: &[(&str, &str)]) -> Utf8PathBuf {
    let dir = conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        fs::write(dir.join(name), content).unwrap();
    }
    dir
}

#[test]
fn trash_moves_the_conversation_and_writes_a_note() {
    let (_tmp, storage, conversations) = single_root();
    write_dir(&conversations, "17941219994-a-title", &[
        (METADATA_FILE, "{}"),
        (EVENTS_FILE, "[]"),
    ]);

    let moved = storage
        .trash_conversation(&id("17941219994"), now())
        .unwrap();

    let trashed = conversations.join(TRASH_DIR).join("17941219994-a-title");
    assert_eq!(moved, [Moved {
        from: conversations.join("17941219994-a-title"),
        to: trashed.clone(),
    }]);
    assert!(!conversations.join("17941219994-a-title").exists());
    assert_eq!(
        fs::read_to_string(trashed.join(METADATA_FILE)).unwrap(),
        "{}"
    );
    assert_eq!(
        fs::read_to_string(trashed.join(TRASHED_FILE)).unwrap(),
        "# Trashed Conversation\n\nMoved here by `jp conversation trash` on \
         2026-11-19T12:00:00Z.\n\nRun `jp conversation untrash jp-c17941219994` to move it back.\n"
    );
}

// The directories JP cannot load are the ones a user most needs to set aside,
// and a draft that was never stored is one too.
#[test]
fn trash_takes_invalid_and_unstored_conversations() {
    let (_tmp, storage, conversations) = single_root();
    write_dir(&conversations, "17941219991", &[(
        METADATA_FILE,
        "not json",
    )]);
    write_dir(&conversations, "17941219992", &[(
        "QUERY_MESSAGE.md",
        "Reply with: ok",
    )]);

    storage
        .trash_conversation(&id("17941219991"), now())
        .unwrap();
    storage
        .trash_conversation(&id("17941219992"), now())
        .unwrap();

    let trash = conversations.join(TRASH_DIR);
    assert!(trash.join("17941219991").join(METADATA_FILE).is_file());
    assert!(trash.join("17941219992").join("QUERY_MESSAGE.md").is_file());
}

// A projected conversation lives in both roots; trashing one copy would leave
// the other live.
#[test]
fn trash_moves_every_copy_across_storage_roots() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("ws"))
        .unwrap()
        .with_user_storage(&tmp.path().join("user"), None, "abc")
        .unwrap();
    let workspace = tmp.path().join("ws").join(CONVERSATIONS_DIR);
    let user = tmp.path().join("user").join("abc").join(CONVERSATIONS_DIR);
    write_dir(&workspace, "17941219994", &[(EVENTS_FILE, "[]")]);
    write_dir(&user, "17941219994", &[(EVENTS_FILE, "[]")]);

    let moved = storage
        .trash_conversation(&id("17941219994"), now())
        .unwrap();

    assert_eq!(moved.len(), 2);
    assert!(workspace.join(TRASH_DIR).join("17941219994").is_dir());
    assert!(user.join(TRASH_DIR).join("17941219994").is_dir());
}

#[test]
fn trash_reports_a_conversation_that_is_not_there() {
    let (_tmp, storage, _conversations) = single_root();

    assert!(matches!(
        storage.trash_conversation(&id("17941219994"), now()),
        Err(TrashError::NotFound)
    ));
}

// One entry per conversation in the trash keeps `untrash` unambiguous, so a
// second copy is refused rather than renamed beside the first.
#[test]
fn trash_refuses_a_conversation_the_trash_already_holds() {
    let (_tmp, storage, conversations) = single_root();
    write_dir(&conversations, "17941219994", &[(EVENTS_FILE, "[]")]);
    let earlier = write_dir(&conversations.join(TRASH_DIR), "17941219994-old", &[(
        EVENTS_FILE,
        "[1]",
    )]);

    let error = storage
        .trash_conversation(&id("17941219994"), now())
        .unwrap_err();

    assert!(matches!(error, TrashError::AlreadyTrashed(ref path) if *path == earlier));
    assert!(conversations.join("17941219994").is_dir());
    assert_eq!(
        fs::read_to_string(earlier.join(EVENTS_FILE)).unwrap(),
        "[1]"
    );
}

#[test]
fn trash_refuses_a_conversation_another_process_holds() {
    let (_tmp, storage, conversations) = single_root();
    write_dir(&conversations, "17941219994", &[(EVENTS_FILE, "[]")]);
    let _lock = storage
        .try_lock_conversation(&id("17941219994").to_string(), None)
        .unwrap()
        .unwrap();

    assert!(matches!(
        storage.trash_conversation(&id("17941219994"), now()),
        Err(TrashError::Locked)
    ));
    assert!(conversations.join("17941219994").is_dir());
}

// A dry run answers what `trash_conversation` would do, and changes nothing.
#[test]
fn plan_trash_moves_nothing() {
    let (_tmp, storage, conversations) = single_root();
    write_dir(&conversations, "17941219994-a-title", &[(
        EVENTS_FILE,
        "[]",
    )]);

    let planned = storage.plan_trash(&id("17941219994")).unwrap();

    assert_eq!(planned, [Moved {
        from: conversations.join("17941219994-a-title"),
        to: conversations.join(TRASH_DIR).join("17941219994-a-title"),
    }]);
    assert!(conversations.join("17941219994-a-title").is_dir());
    assert!(!conversations.join(TRASH_DIR).exists());
}

#[test]
fn plan_trash_reports_what_trash_would_refuse() {
    let (_tmp, storage, conversations) = single_root();
    assert!(matches!(
        storage.plan_trash(&id("17941219994")),
        Err(TrashError::NotFound)
    ));

    write_dir(&conversations, "17941219994", &[(EVENTS_FILE, "[]")]);
    let _lock = storage
        .try_lock_conversation(&id("17941219994").to_string(), None)
        .unwrap()
        .unwrap();
    assert!(matches!(
        storage.plan_trash(&id("17941219994")),
        Err(TrashError::Locked)
    ));
}

#[test]
fn plan_untrash_moves_nothing() {
    let (_tmp, storage, conversations) = single_root();
    let trashed = write_dir(&conversations.join(TRASH_DIR), "17941219994", &[(
        EVENTS_FILE,
        "[]",
    )]);

    let planned = storage.plan_untrash(&id("17941219994")).unwrap();

    assert_eq!(planned, [Moved {
        from: trashed.clone(),
        to: conversations.join("17941219994"),
    }]);
    assert!(trashed.is_dir());
    assert!(!conversations.join("17941219994").exists());
}

#[test]
fn plan_untrash_reports_what_untrash_would_refuse() {
    let (_tmp, storage, conversations) = single_root();
    write_dir(&conversations.join(TRASH_DIR), "17941219994", &[(
        EVENTS_FILE,
        "[]",
    )]);
    let live = write_dir(&conversations, "17941219994-new", &[(EVENTS_FILE, "[]")]);

    assert!(matches!(
        storage.plan_untrash(&id("17941219994")),
        Err(TrashError::Exists(ref path)) if *path == live
    ));
}

#[test]
fn untrash_moves_the_conversation_back_without_the_note() {
    let (_tmp, storage, conversations) = single_root();
    write_dir(&conversations, "17941219994-a-title", &[(
        EVENTS_FILE,
        "[]",
    )]);
    storage
        .trash_conversation(&id("17941219994"), now())
        .unwrap();

    let moved = storage.untrash_conversation(&id("17941219994")).unwrap();

    let live = conversations.join("17941219994-a-title");
    assert_eq!(moved, [Moved {
        from: conversations.join(TRASH_DIR).join("17941219994-a-title"),
        to: live.clone(),
    }]);
    assert_eq!(fs::read_to_string(live.join(EVENTS_FILE)).unwrap(), "[]");
    assert!(!live.join(TRASHED_FILE).exists());
}

#[test]
fn untrash_refuses_to_replace_a_live_conversation() {
    let (_tmp, storage, conversations) = single_root();
    let trashed = write_dir(&conversations.join(TRASH_DIR), "17941219994", &[(
        EVENTS_FILE,
        "[1]",
    )]);
    let live = write_dir(&conversations, "17941219994-new", &[(EVENTS_FILE, "[2]")]);

    let error = storage
        .untrash_conversation(&id("17941219994"))
        .unwrap_err();

    assert!(matches!(error, TrashError::Exists(ref path) if *path == live));
    assert_eq!(
        fs::read_to_string(trashed.join(EVENTS_FILE)).unwrap(),
        "[1]"
    );
    assert_eq!(fs::read_to_string(live.join(EVENTS_FILE)).unwrap(), "[2]");
}

#[test]
fn untrash_reports_a_conversation_that_is_not_trashed() {
    let (_tmp, storage, conversations) = single_root();
    write_dir(&conversations, "17941219994", &[(EVENTS_FILE, "[]")]);

    assert!(matches!(
        storage.untrash_conversation(&id("17941219994")),
        Err(TrashError::NotTrashed)
    ));
}

#[test]
fn untrash_refuses_a_conversation_another_process_holds() {
    let (_tmp, storage, conversations) = single_root();
    let trashed = write_dir(&conversations.join(TRASH_DIR), "17941219994", &[(
        EVENTS_FILE,
        "[]",
    )]);
    let _lock = storage
        .try_lock_conversation(&id("17941219994").to_string(), None)
        .unwrap()
        .unwrap();

    assert!(matches!(
        storage.untrash_conversation(&id("17941219994")),
        Err(TrashError::Locked)
    ));
    assert!(trashed.is_dir());
}
