use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_conversation::ConversationId;
use jp_storage::backend::{FsStorageBackend, LockBackend as _};

use super::*;

/// The `TRASHED.md` startup validation wrote beside every conversation it moved
/// to the trash.
const AUTO_TRASHED_MD: &str = "# Trashed Conversation

This conversation was moved here because it failed workspace sanitization.
";

const EVENTS: &str = r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request"}]"#;

fn setup() -> (Utf8TempDir, FsStorageBackend, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let backend = FsStorageBackend::new(&tmp.path().join(".jp")).unwrap();
    let conversations = tmp.path().join(".jp").join("conversations");
    fs::create_dir_all(&conversations).unwrap();
    (tmp, backend, conversations)
}

/// Write a trash entry as startup validation left it, holding `files`.
fn trashed(conversations: &Utf8Path, dirname: &str, files: &[(&str, &str)]) -> Utf8PathBuf {
    let dir = conversations.join(".trash").join(dirname);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("TRASHED.md"), AUTO_TRASHED_MD).unwrap();
    for (name, content) in files {
        fs::write(dir.join(name), content).unwrap();
    }
    dir
}

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

// Whatever an entry holds, it goes back: what fails to load is reported by the
// load-failure report, and what is not a conversation by `jp doctor`.
#[test]
fn moves_every_entry_back() {
    let (_tmp, backend, conversations) = setup();
    trashed(&conversations, "17941219991", &[
        ("metadata.json", "{}"),
        ("events.json", EVENTS),
    ]);
    trashed(&conversations, "17941219992", &[(
        "QUERY_MESSAGE.md",
        "Reply with: ok",
    )]);
    trashed(&conversations, "17941219993", &[(
        "metadata.json",
        "not json",
    )]);
    trashed(&conversations, "not-an-id", &[("notes", "kept")]);

    restore_trash(&backend);

    assert_eq!(
        fs::read_to_string(conversations.join("17941219991/events.json")).unwrap(),
        EVENTS
    );
    assert_eq!(
        fs::read_to_string(conversations.join("17941219992/QUERY_MESSAGE.md")).unwrap(),
        "Reply with: ok"
    );
    assert_eq!(
        fs::read_to_string(conversations.join("17941219993/metadata.json")).unwrap(),
        "not json"
    );
    assert_eq!(
        fs::read_to_string(conversations.join("not-an-id/notes")).unwrap(),
        "kept"
    );
    for dirname in ["17941219991", "17941219992", "17941219993", "not-an-id"] {
        assert!(!conversations.join(dirname).join("TRASHED.md").exists());
    }
    assert!(!conversations.join(".trash").exists());
}

// Two directories for one conversation would leave storage to pick one of
// them, so an entry whose conversation is live stays where it is.
#[test]
fn never_moves_an_entry_beside_a_live_conversation() {
    let (_tmp, backend, conversations) = setup();
    let live = conversations.join("17941219994-a-title");
    fs::create_dir_all(&live).unwrap();
    fs::write(live.join("events.json"), EVENTS).unwrap();
    let entry = trashed(&conversations, "17941219994", &[("events.json", "[]")]);
    trashed(&conversations, "17941219995", &[("events.json", "[]")]);

    restore_trash(&backend);

    assert_eq!(
        fs::read_to_string(live.join("events.json")).unwrap(),
        EVENTS
    );
    assert_eq!(fs::read_to_string(entry.join("events.json")).unwrap(), "[]");
    assert!(entry.join("TRASHED.md").is_file());
    assert!(!conversations.join("17941219994").exists());
    assert!(conversations.join("17941219995/events.json").is_file());
}

// Another process holding the lock may be about to create the conversation, so
// the entry waits, and the next run moves it.
#[test]
fn a_locked_conversation_moves_on_a_later_run() {
    let (_tmp, backend, conversations) = setup();
    let entry = trashed(&conversations, "17941219991", &[("events.json", EVENTS)]);

    let lock = backend
        .try_lock(&id("17941219991").to_string(), None)
        .unwrap()
        .unwrap();
    restore_trash(&backend);
    assert!(entry.join("events.json").is_file());
    assert!(!conversations.join("17941219991").exists());

    drop(lock);
    restore_trash(&backend);
    assert!(conversations.join("17941219991/events.json").is_file());
    assert!(!conversations.join(".trash").exists());
}

#[test]
fn a_root_without_a_trash_is_left_alone() {
    let (_tmp, backend, conversations) = setup();

    restore_trash(&backend);

    assert!(!conversations.join(".trash").exists());
    assert_eq!(fs::read_dir(&conversations).unwrap().count(), 0);
}

// User-local storage has a trash of its own.
#[test]
fn restores_the_trash_of_every_storage_root() {
    let tmp = tempdir().unwrap();
    let backend = FsStorageBackend::new(&tmp.path().join(".jp"))
        .unwrap()
        .with_user_storage(&tmp.path().join("user"), None, "abc")
        .unwrap();
    let [workspace, user] = <[Utf8PathBuf; 2]>::try_from(backend.conversations_dirs()).unwrap();
    trashed(&workspace, "17941219991", &[("events.json", EVENTS)]);
    trashed(&user, "17941219992", &[("events.json", EVENTS)]);

    restore_trash(&backend);

    assert!(workspace.join("17941219991/events.json").is_file());
    assert!(user.join("17941219992/events.json").is_file());
}
