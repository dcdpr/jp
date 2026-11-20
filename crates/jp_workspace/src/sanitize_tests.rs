use std::{fs, sync::Arc};

use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use datetime_literal::datetime;
use jp_conversation::{Conversation, ConversationId};
use jp_storage::{
    backend::{FsStorageBackend, LockBackend as _, SanitizeReport},
    validate::InvalidConversation,
};
use test_log::test;

use crate::Workspace;

/// Create an `FsStorageBackend` for fixture setup and a `Workspace` pointing to
/// the same storage path.
fn setup() -> (Utf8TempDir, Arc<FsStorageBackend>, Workspace) {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let fs = Arc::new(FsStorageBackend::new(&storage_path).unwrap());
    let mut ws = Workspace::in_memory(tmp.path()).with_backend(fs.clone());
    ws.disable_persistence();
    (tmp, fs, ws)
}

fn conversation_dir(backend: &FsStorageBackend, dirname: &str) -> Utf8PathBuf {
    backend.storage_path().join("conversations").join(dirname)
}

fn reported_paths(report: &SanitizeReport) -> Vec<Utf8PathBuf> {
    report
        .invalid
        .iter()
        .map(InvalidConversation::path)
        .collect()
}

#[test]
fn test_clean_workspace_reports_nothing() {
    let (_tmp, fs, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id, &Conversation::default());

    let report = ws.sanitize().unwrap();

    assert!(report.invalid.is_empty());
}

// JP reports a broken conversation and leaves its files where they are: moving
// a user's data is the user's decision.
#[test]
fn test_invalid_conversations_are_reported_and_left_in_place() {
    let (_tmp, fs, mut ws) = setup();

    let valid_id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&valid_id, &Conversation::default());

    // A directory whose name is not a conversation ID is not a conversation, so
    // only the corrupt conversation is reported, and neither moves.
    let bad_id = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    fs.create_corrupt_test_conversation_dir(&bad_id.to_dirname(None))
        .unwrap();
    fs.create_corrupt_test_conversation_dir("not-a-valid-id")
        .unwrap();

    let report = ws.sanitize().unwrap();

    assert_eq!(reported_paths(&report), [conversation_dir(
        &fs,
        "17041536000"
    )]);
    assert!(
        conversation_dir(&fs, "17041536000")
            .join("metadata.json")
            .is_file()
    );
    assert!(
        conversation_dir(&fs, "not-a-valid-id")
            .join("metadata.json")
            .is_file()
    );
    assert!(!conversation_dir(&fs, ".trash").exists());
}

#[test]
fn test_invalid_conversations_are_not_indexed() {
    let (_tmp, fs, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id2, &Conversation::default());
    fs.create_corrupt_test_conversation_dir(&id1.to_dirname(None))
        .unwrap();

    ws.sanitize().unwrap();
    ws.load_conversation_index();

    assert!(ws.acquire_conversation(&id2).is_ok());
    assert!(ws.acquire_conversation(&id1).is_err());
}

// The index keeps a broken conversation out on its own, so a reader that never
// sanitizes, such as the macOS app, does not hand it on to be loaded either.
#[test]
fn test_invalid_conversations_are_not_indexed_without_sanitize() {
    let (_tmp, fs, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id2, &Conversation::default());
    fs.create_corrupt_test_conversation_dir(&id1.to_dirname(None))
        .unwrap();

    ws.load_conversation_index();

    assert!(ws.acquire_conversation(&id2).is_ok());
    assert!(ws.acquire_conversation(&id1).is_err());
}

#[test]
fn test_load_index_populates_all_conversations() {
    let (_tmp, fs, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id1, &Conversation::default());
    fs.write_test_conversation(&id2, &Conversation::default());

    ws.sanitize().unwrap();
    ws.load_conversation_index();

    // Both conversations should be in the index.
    assert!(ws.acquire_conversation(&id1).is_ok());
    assert!(ws.acquire_conversation(&id2).is_ok());
}

#[test]
fn test_eager_load_populates_metadata_and_events() {
    let (_tmp, fs, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id1, &Conversation::default());
    fs.write_test_conversation(&id2, &Conversation::default());

    ws.sanitize().unwrap();
    ws.load_conversation_index();
    let handle = ws.acquire_conversation(&id1).unwrap();
    ws.eager_load_conversation(&handle).unwrap();
    // Metadata and events should be available without lazy-loading.
    assert_eq!(ws.metadata(&handle).unwrap().title, None);
    assert!(ws.events(&handle).unwrap().is_empty());
}

#[test]
fn test_eager_load_missing_conversation_errors() {
    let (_tmp, _fs, mut ws) = setup();

    let missing = ConversationId::try_from(datetime!(2024-06-01 00:00:00 Z)).unwrap();
    ws.load_conversation_index();

    // Can't even acquire a handle for a missing conversation.
    assert!(ws.acquire_conversation(&missing).is_err());
}

#[test]
fn test_only_invalid_conversations_produce_an_empty_index() {
    let (_tmp, fs, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.create_corrupt_test_conversation_dir(&id.to_dirname(None))
        .unwrap();

    let report = ws.sanitize().unwrap();
    assert_eq!(report.invalid.len(), 1);

    ws.load_conversation_index();
    assert_eq!(ws.conversations().count(), 0);
}

// The shape an older `jp query --new` left when interrupted: the request the
// user typed, in a directory the conversation's first write never reached. It
// is reported, and the draft stays where it is.
#[test]
fn test_a_draft_without_conversation_files_is_reported_and_left_in_place() {
    let (_tmp, backend, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let dir = backend.build_conversation_dir(&id, None, true);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("QUERY_MESSAGE.md"), "typed request").unwrap();

    let report = ws.sanitize().unwrap();
    assert_eq!(
        report
            .invalid
            .iter()
            .map(|invalid| (invalid.id, invalid.error.to_string()))
            .collect::<Vec<_>>(),
        [(id, "missing metadata".to_owned())]
    );
    assert_eq!(
        fs::read_to_string(dir.join("QUERY_MESSAGE.md")).unwrap(),
        "typed request"
    );

    ws.load_conversation_index();
    assert_eq!(ws.conversations().count(), 0);
}

// Another process may be between the writes of the conversation's files.
#[test]
fn test_a_locked_conversation_without_files_is_not_reported() {
    let (_tmp, backend, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let dir = backend.build_conversation_dir(&id, None, true);
    fs::create_dir_all(&dir).unwrap();
    let _held = backend.try_lock(&id.to_string(), None).unwrap().unwrap();

    assert!(ws.sanitize().unwrap().invalid.is_empty());
}

#[test]
fn test_skips_dot_prefixed_directories() {
    let (_tmp, fs, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id, &Conversation::default());

    fs.create_corrupt_test_conversation_dir(".trash").unwrap();
    fs.create_corrupt_test_conversation_dir(".hidden").unwrap();

    let report = ws.sanitize().unwrap();
    assert!(report.invalid.is_empty());
}

#[test]
fn test_no_storage_returns_empty_report() {
    // Without filesystem storage, sanitize returns an empty report
    // (InMemoryStorageBackend has nothing to sanitize).
    let mut ws = Workspace::in_memory("/nonexistent");
    let report = ws.sanitize().unwrap();
    assert!(report.invalid.is_empty());
}

#[test]
fn test_empty_workspace_no_conversations_dir() {
    let (_tmp, _fs, mut ws) = setup();
    let report = ws.sanitize().unwrap();
    assert!(report.invalid.is_empty());
}

// A conversation another process holds may be halfway through a write, so its
// files are not yet worth reporting on.
#[test]
fn test_locked_invalid_conversation_is_not_reported() {
    let (_tmp, fs, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.create_corrupt_test_conversation_dir(&id.to_dirname(None))
        .unwrap();

    let lock = fs.try_lock(&id.to_string(), None).unwrap().unwrap();
    let report = ws.sanitize().unwrap();
    assert!(report.invalid.is_empty());

    drop(lock);
    let report = ws.sanitize().unwrap();
    assert_eq!(reported_paths(&report), [conversation_dir(
        &fs,
        "17040672000"
    )]);
}

#[test]
fn test_sanitize_then_load_with_mixed_valid_and_invalid() {
    let (_tmp, fs, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    let id3 = ConversationId::try_from(datetime!(2024-01-03 00:00:00 Z)).unwrap();
    let id4 = ConversationId::try_from(datetime!(2024-01-04 00:00:00 Z)).unwrap();

    fs.write_test_conversation(&id1, &Conversation::default());
    fs.write_test_conversation(&id3, &Conversation::default());

    // id2 and id4 are invalid (corrupt metadata).
    fs.create_corrupt_test_conversation_dir(&id2.to_dirname(None))
        .unwrap();
    fs.create_corrupt_test_conversation_dir(&id4.to_dirname(None))
        .unwrap();

    let report = ws.sanitize().unwrap();
    assert_eq!(report.invalid.len(), 2);

    ws.load_conversation_index();
    // Only valid conversations should be in the index.
    assert!(ws.acquire_conversation(&id1).is_ok());
    assert!(ws.acquire_conversation(&id3).is_ok());
    assert!(ws.acquire_conversation(&id2).is_err());
    assert!(ws.acquire_conversation(&id4).is_err());
}
