use std::sync::Arc;

use camino_tempfile::{Utf8TempDir, tempdir};
use datetime_literal::datetime;
use jp_conversation::{Conversation, ConversationId};
use jp_storage::backend::FsStorageBackend;
use test_log::test;

use crate::Workspace;

/// Create an `FsStorageBackend` for fixture setup and a `Workspace` pointing to
/// the same storage path.
fn setup() -> (Utf8TempDir, Arc<FsStorageBackend>, Workspace) {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let fs = Arc::new(FsStorageBackend::new(&storage_path).unwrap());
    let mut ws = Workspace::new(tmp.path()).with_backend(fs.clone());
    ws.disable_persistence();
    (tmp, fs, ws)
}

#[test]
fn test_clean_workspace_reports_no_repairs() {
    let (_tmp, fs, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id, &Conversation::default());

    let report = ws.sanitize().unwrap();

    assert!(!report.has_repairs());
    assert!(report.trashed.is_empty());
}

#[test]
fn test_invalid_conversations_are_trashed() {
    let (_tmp, fs, mut ws) = setup();

    let valid_id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&valid_id, &Conversation::default());

    // Two different kinds of invalid: bad dirname + empty dir with valid ID.
    let bad_id = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    fs.create_test_conversation_dir("not-a-valid-id");
    fs.create_test_conversation_dir(&bad_id.to_dirname(None));

    let report = ws.sanitize().unwrap();
    assert_eq!(report.trashed.len(), 2);
}

#[test]
fn test_trashed_conversations_not_loaded() {
    let (_tmp, fs, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id2, &Conversation::default());

    // id1 is invalid (empty dir).
    fs.create_test_conversation_dir(&id1.to_dirname(None));

    let report = ws.sanitize().unwrap();
    assert_eq!(report.trashed.len(), 1);

    // After sanitize + load, only the valid conversation should be accessible.
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
fn test_all_conversations_trashed_produces_empty_workspace() {
    let (_tmp, fs, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.create_test_conversation_dir(&id.to_dirname(None));

    let report = ws.sanitize().unwrap();
    assert_eq!(report.trashed.len(), 1);

    // Fresh workspace after all trashed.
    ws.load_conversation_index();
    assert_eq!(ws.conversations().count(), 0);
}

#[test]
fn test_skips_dot_prefixed_directories() {
    let (_tmp, fs, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    fs.write_test_conversation(&id, &Conversation::default());

    // These should be silently ignored, not trashed.
    fs.create_test_conversation_dir(".trash");
    fs.create_test_conversation_dir(".hidden");

    let report = ws.sanitize().unwrap();
    assert!(!report.has_repairs());
}

#[test]
fn test_no_storage_returns_empty_report() {
    // Without filesystem storage, sanitize returns an empty report
    // (InMemoryStorageBackend has nothing to sanitize).
    let mut ws = Workspace::new("/nonexistent");
    let report = ws.sanitize().unwrap();
    assert!(!report.has_repairs());
}

#[test]
fn test_empty_workspace_no_conversations_dir() {
    let (_tmp, _fs, mut ws) = setup();
    let report = ws.sanitize().unwrap();
    assert!(!report.has_repairs());
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

    // id2 and id4 are invalid (empty dirs).
    fs.create_test_conversation_dir(&id2.to_dirname(None));
    fs.create_test_conversation_dir(&id4.to_dirname(None));

    let report = ws.sanitize().unwrap();
    assert_eq!(report.trashed.len(), 2);

    ws.load_conversation_index();
    // Only valid conversations should be in the index.
    assert!(ws.acquire_conversation(&id1).is_ok());
    assert!(ws.acquire_conversation(&id3).is_ok());
    assert!(ws.acquire_conversation(&id2).is_err());
    assert!(ws.acquire_conversation(&id4).is_err());
}
