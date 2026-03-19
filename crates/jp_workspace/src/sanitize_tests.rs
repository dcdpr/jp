use std::sync::Arc;

use camino_tempfile::tempdir;
use datetime_literal::datetime;
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId};
use jp_storage::Storage;
use test_log::test;

use crate::Workspace;

fn test_config() -> Arc<AppConfig> {
    Arc::new(AppConfig::new_test())
}

/// Create a Storage for fixture setup and a Workspace pointing to the same
/// storage path.
fn setup() -> (camino_tempfile::Utf8TempDir, Storage, Workspace) {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let setup = Storage::new(&storage_path).unwrap();
    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap();
    ws.disable_persistence();
    (tmp, setup, ws)
}

#[test]
fn test_clean_workspace_reports_no_repairs() {
    let (_tmp, storage, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    storage.write_test_conversation(&id, &Conversation::default());
    storage.write_test_conversations_metadata(id);

    let report = ws.sanitize().unwrap();

    assert!(!report.has_repairs());
    assert!(report.trashed.is_empty());
    assert!(!report.active_reassigned);
    assert!(!report.default_created);
}

#[test]
fn test_invalid_conversations_are_trashed() {
    let (_tmp, storage, mut ws) = setup();

    let valid_id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    storage.write_test_conversation(&valid_id, &Conversation::default());
    storage.write_test_conversations_metadata(valid_id);

    // Two different kinds of invalid: bad dirname + empty dir with valid ID.
    let bad_id = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    storage.create_test_conversation_dir("not-a-valid-id");
    storage.create_test_conversation_dir(&bad_id.to_dirname(None));

    let report = ws.sanitize().unwrap();

    assert_eq!(report.trashed.len(), 2);
    assert!(!report.active_reassigned);
    assert!(!report.default_created);
}

#[test]
fn test_reassigns_active_when_trashed() {
    let (_tmp, storage, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    storage.write_test_conversation(&id2, &Conversation::default());

    // id1 is active but invalid (empty dir, will fail validation).
    storage.create_test_conversation_dir(&id1.to_dirname(None));
    storage.write_test_conversations_metadata(id1);

    let report = ws.sanitize().unwrap();

    assert!(report.active_reassigned);
    assert!(!report.default_created);

    ws.load_conversation_index().unwrap();
    assert_eq!(ws.active_conversation_id(), id2);
}

#[test]
fn test_reassigns_to_most_recent_valid() {
    let (_tmp, storage, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    let id3 = ConversationId::try_from(datetime!(2024-01-03 00:00:00 Z)).unwrap();
    storage.write_test_conversation(&id1, &Conversation::default());
    storage.write_test_conversation(&id2, &Conversation::default());

    // id3 is active but invalid.
    storage.create_test_conversation_dir(&id3.to_dirname(None));
    storage.write_test_conversations_metadata(id3);

    let report = ws.sanitize().unwrap();

    assert!(report.active_reassigned);
    ws.load_conversation_index().unwrap();
    assert_eq!(ws.active_conversation_id(), id2);
}

#[test]
fn test_reassigns_when_active_has_no_directory() {
    let (_tmp, storage, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    let id3 = ConversationId::try_from(datetime!(2024-01-03 00:00:00 Z)).unwrap();
    storage.write_test_conversation(&id1, &Conversation::default());
    storage.write_test_conversation(&id2, &Conversation::default());

    // Metadata points to id3 which has no directory at all.
    storage.write_test_conversations_metadata(id3);

    let report = ws.sanitize().unwrap();

    assert!(report.trashed.is_empty());
    assert!(report.active_reassigned);
    assert!(!report.default_created);

    ws.load_conversation_index().unwrap();
    assert_eq!(ws.active_conversation_id(), id2);
}

#[test]
fn test_default_created_when_all_trashed() {
    let (_tmp, storage, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();

    // Only conversation is invalid.
    storage.create_test_conversation_dir(&id.to_dirname(None));
    storage.write_test_conversations_metadata(id);

    let report = ws.sanitize().unwrap();

    assert!(report.active_reassigned);
    assert!(report.default_created);
    assert_eq!(report.trashed.len(), 1);

    ws.load_conversation_index().unwrap();
    ws.ensure_active_conversation_stream(test_config()).unwrap();
}

#[test]
fn test_corrupt_global_metadata_reassigns_to_valid() {
    let (_tmp, storage, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    storage.write_test_conversation(&id1, &Conversation::default());
    storage.write_test_conversation(&id2, &Conversation::default());
    storage.write_test_corrupt_conversations_metadata();

    let report = ws.sanitize().unwrap();

    assert!(report.trashed.is_empty());
    assert!(report.active_reassigned);
    assert!(!report.default_created);

    ws.load_conversation_index().unwrap();
    assert_eq!(ws.active_conversation_id(), id2);
}

#[test]
fn test_corrupt_global_metadata_with_no_conversations() {
    let (_tmp, storage, mut ws) = setup();

    storage.write_test_corrupt_conversations_metadata();

    let report = ws.sanitize().unwrap();

    // Empty workspace with corrupt metadata is silently repaired.
    assert!(!report.has_repairs());
    assert!(!storage.conversations_metadata_exists());

    ws.load_conversation_index().unwrap();
    ws.ensure_active_conversation_stream(test_config()).unwrap();
}

#[test]
fn test_skips_dot_prefixed_directories() {
    let (_tmp, storage, mut ws) = setup();

    let id = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    storage.write_test_conversation(&id, &Conversation::default());
    storage.write_test_conversations_metadata(id);

    // These should be silently ignored, not trashed.
    storage.create_test_conversation_dir(".trash");
    storage.create_test_conversation_dir(".hidden");

    let report = ws.sanitize().unwrap();
    assert!(!report.has_repairs());
}

#[test]
fn test_no_storage_returns_error() {
    let mut ws = Workspace::new("/nonexistent");
    assert!(ws.sanitize().is_err());
}

#[test]
fn test_empty_workspace_no_conversations_dir() {
    let (_tmp, _storage, mut ws) = setup();
    let report = ws.sanitize().unwrap();
    assert!(!report.has_repairs());
}

#[test]
fn test_sanitize_then_load_with_mixed_valid_and_invalid() {
    let (_tmp, storage, mut ws) = setup();

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();
    let id3 = ConversationId::try_from(datetime!(2024-01-03 00:00:00 Z)).unwrap();
    let id4 = ConversationId::try_from(datetime!(2024-01-04 00:00:00 Z)).unwrap();

    storage.write_test_conversation(&id1, &Conversation::default());
    storage.write_test_conversation(&id3, &Conversation::default());

    // id2 and id4 are invalid (empty dirs).
    storage.create_test_conversation_dir(&id2.to_dirname(None));
    storage.create_test_conversation_dir(&id4.to_dirname(None));

    // Active points to id4 (invalid).
    storage.write_test_conversations_metadata(id4);

    let report = ws.sanitize().unwrap();

    assert_eq!(report.trashed.len(), 2);
    assert!(report.active_reassigned);
    assert!(!report.default_created);

    ws.load_conversation_index().unwrap();
    assert_eq!(ws.active_conversation_id(), id3);
    assert!(ws.get_conversation(&id1).is_some());
}
