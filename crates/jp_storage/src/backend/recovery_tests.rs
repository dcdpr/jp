use std::{fs, slice, sync::Arc};

use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_conversation::{Conversation, ConversationId};
use test_log::test;

use super::*;
use crate::{
    backend::{FsStorageBackend, LockBackend as _},
    validate::{ConversationPart, StorageLocation, ValidationError},
};

fn setup() -> (Utf8TempDir, Arc<FsStorageBackend>, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let storage = tmp.path().join("storage");
    let fs = Arc::new(FsStorageBackend::new(&storage).unwrap());
    let conversations = storage.join("conversations");
    (tmp, fs, conversations)
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

fn scanned_paths(fs: &FsStorageBackend) -> Vec<Utf8PathBuf> {
    fs.scan()
        .unwrap()
        .iter()
        .map(InvalidConversation::path)
        .collect()
}

#[test]
fn a_store_of_valid_conversations_scans_clean() {
    let (_tmp, fs, _conversations) = setup();
    fs.write_test_conversation(&id("17041536000"), &Conversation::default());

    assert!(fs.scan().unwrap().is_empty());
}

// JP reports a broken conversation and leaves its files where they are: moving
// a user's data is the user's decision.
#[test]
fn invalid_conversations_are_reported_and_left_in_place() {
    let (_tmp, fs, conversations) = setup();
    fs.write_test_conversation(&id("17040672000"), &Conversation::default());
    let bad = broken(&conversations, "17041536000");

    // A directory whose name is not a conversation ID is not a conversation.
    let other = broken(&conversations, "not-a-valid-id");

    assert_eq!(scanned_paths(&fs), slice::from_ref(&bad));
    assert!(bad.join("metadata.json").is_file());
    assert!(other.join("metadata.json").is_file());
    assert!(!conversations.join(".trash").exists());
}

#[test]
fn dot_prefixed_directories_are_not_scanned() {
    let (_tmp, fs, conversations) = setup();
    broken(&conversations, ".trash");
    broken(&conversations, ".hidden");

    assert!(fs.scan().unwrap().is_empty());
}

#[test]
fn a_store_without_conversations_scans_clean() {
    let (_tmp, fs, _conversations) = setup();

    assert!(fs.scan().unwrap().is_empty());
}

// A conversation another process holds may be between the writes of its files,
// so its state says nothing yet.
#[test]
fn a_locked_conversation_is_not_reported_until_released() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17040672000");

    let lock = fs.try_lock("jp-c17040672000", None).unwrap().unwrap();
    assert!(fs.scan().unwrap().is_empty());

    drop(lock);
    assert_eq!(scanned_paths(&fs), [dir]);
}

#[test]
fn inspect_reports_why_a_conversation_does_not_load() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");

    let found = fs.inspect(&id("17941219991")).unwrap();

    assert_eq!(found.locations, [StorageLocation::new(dir.as_str())]);
    assert!(matches!(
        found.state,
        Err(ValidationError::Unreadable {
            part: ConversationPart::Metadata,
            ..
        })
    ));
}

#[test]
fn inspect_finds_nothing_for_an_id_without_a_directory() {
    let (_tmp, fs, _conversations) = setup();

    assert!(fs.inspect(&id("17941219991")).is_none());
}

#[test]
fn removal_deletes_every_directory_of_the_conversation() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991-a-title");

    let mut shown = vec![];
    let outcome = fs
        .remove(&id("17941219991"), &mut |found| {
            shown.clone_from(&found.locations);
            true
        })
        .unwrap();

    assert_eq!(outcome, Removal::Removed);
    assert_eq!(shown, [StorageLocation::new(dir.as_str())]);
    assert!(!dir.exists());
}

#[test]
fn a_declined_removal_keeps_the_conversation() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");

    let outcome = fs.remove(&id("17941219991"), &mut |_| false).unwrap();

    assert_eq!(outcome, Removal::Declined);
    assert!(dir.is_dir());
}

// The decision is never asked for while another process holds the
// conversation, so nothing it shows can change before the removal.
#[test]
fn a_conversation_another_process_holds_is_not_removed() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");
    let _held = fs.try_lock("jp-c17941219991", None).unwrap().unwrap();

    let outcome = fs
        .remove(&id("17941219991"), &mut |_| panic!("not asked"))
        .unwrap();

    assert_eq!(outcome, Removal::Locked);
    assert!(dir.is_dir());
}

#[test]
fn removing_an_id_without_a_directory_finds_nothing() {
    let (_tmp, fs, _conversations) = setup();

    let outcome = fs
        .remove(&id("17941219991"), &mut |_| panic!("not asked"))
        .unwrap();

    assert_eq!(outcome, Removal::NotFound);
}

// A dry run asks the same question and reports the same answer, and changes
// nothing on disk.
#[test]
fn a_dry_run_asks_and_removes_nothing() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");
    let dry = DryRunRecovery::new(fs);

    let mut asked = false;
    let outcome = dry
        .remove(&id("17941219991"), &mut |_| {
            asked = true;
            true
        })
        .unwrap();

    assert!(asked);
    assert_eq!(outcome, Removal::Removed);
    assert!(dir.is_dir());
    assert_eq!(dry.scan().unwrap().len(), 1);
}

#[test]
fn the_null_backend_holds_nothing() {
    let null = NullRecoveryBackend;

    assert!(null.scan().unwrap().is_empty());
    assert!(null.inspect(&id("17941219991")).is_none());
    assert_eq!(
        null.remove(&id("17941219991"), &mut |_| panic!("not asked"))
            .unwrap(),
        Removal::NotFound
    );
}
