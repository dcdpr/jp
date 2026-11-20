use std::{fs, sync::Arc};

use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_conversation::{Conversation, ConversationId};
use jp_storage::{
    backend::{FsStorageBackend, LockBackend as _, NullLockBackend, NullPersistBackend},
    validate::{ConversationPart, StorageLocation, ValidationError},
};
use test_log::test;

use crate::Workspace;

fn setup() -> (Utf8TempDir, Arc<FsStorageBackend>, Workspace, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let storage = tmp.path().join("storage");
    let fs = Arc::new(FsStorageBackend::new(&storage).unwrap());
    let ws = Workspace::in_memory(tmp.path()).with_backend(fs.clone());
    let conversations = storage.join("conversations");
    (tmp, fs, ws, conversations)
}

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

fn broken(conversations: &Utf8PathBuf, dirname: &str) -> Utf8PathBuf {
    let dir = conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("metadata.json"), "not json").unwrap();
    dir
}

#[test]
fn a_conversation_that_fails_to_load_is_unlisted() {
    let (_tmp, _fs, ws, conversations) = setup();
    let dir = broken(&conversations, "17941219991");

    let found = ws.unlisted_conversation(&id("17941219991")).unwrap();

    assert_eq!(found.locations, [StorageLocation::new(dir.as_str())]);
    assert!(matches!(
        found.state,
        Err(ValidationError::Unreadable {
            part: ConversationPart::Metadata,
            ..
        })
    ));
}

// A conversation in the index is reached through the index, not this.
#[test]
fn a_listed_conversation_is_not_unlisted() {
    let (_tmp, fs, mut ws, _conversations) = setup();
    fs.write_test_conversation(&id("17941219991"), &Conversation::default());
    ws.load_conversation_index();

    assert!(ws.unlisted_conversation(&id("17941219991")).is_none());
}

#[test]
fn removing_an_unlisted_conversation_deletes_its_directories() {
    let (_tmp, _fs, ws, conversations) = setup();
    let dir = broken(&conversations, "17941219991-a-title");

    let lock = ws
        .lock_unlisted_conversation(&id("17941219991"))
        .unwrap()
        .unwrap();
    assert_eq!(lock.conversation().locations, [StorageLocation::new(
        dir.as_str()
    )]);
    ws.remove_unlisted_conversation(lock).unwrap();

    assert!(!dir.exists());
}

#[test]
fn a_conversation_another_process_holds_cannot_be_locked() {
    let (_tmp, fs, ws, conversations) = setup();
    let dir = broken(&conversations, "17941219991");
    let _held = fs
        .try_lock(&id("17941219991").to_string(), None)
        .unwrap()
        .unwrap();

    assert!(
        ws.lock_unlisted_conversation(&id("17941219991"))
            .unwrap()
            .is_none()
    );
    assert!(dir.is_dir());
}

// A workspace that persists nothing removes nothing, the same as for a
// conversation that loads.
#[test]
fn a_workspace_that_persists_nothing_removes_nothing() {
    let (_tmp, _fs, ws, conversations) = setup();
    let ws = ws
        .with_persist(Arc::new(NullPersistBackend))
        .with_locker(Arc::new(NullLockBackend));
    let dir = broken(&conversations, "17941219991");

    let lock = ws
        .lock_unlisted_conversation(&id("17941219991"))
        .unwrap()
        .unwrap();
    ws.remove_unlisted_conversation(lock).unwrap();

    assert!(dir.is_dir());
}
