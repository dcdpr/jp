use chrono::{TimeZone as _, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream};

use super::*;
use crate::backend::{
    ConversationFilter, LoadBackend, LockBackend, PersistBackend, SessionBackend,
};

fn test_id(secs: i64) -> ConversationId {
    ConversationId::try_from(Utc.timestamp_opt(secs, 0).unwrap()).unwrap()
}

#[test]
fn persist_write_and_load() {
    let backend = InMemoryStorageBackend::new();
    let id = test_id(1_000_000);
    let meta = Conversation::default();
    let events = ConversationStream::new_test();

    backend.write(&id, &meta, &events).unwrap();

    let loaded_meta = backend.load_conversation_metadata(&id).unwrap();
    assert_eq!(loaded_meta.title, meta.title);

    let loaded_events = backend.load_conversation_stream(&id).unwrap();
    assert!(loaded_events.is_empty());
}

#[test]
fn persist_remove() {
    let backend = InMemoryStorageBackend::new();
    let id = test_id(1_000_000);

    backend
        .write(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
        )
        .unwrap();

    backend.remove(&id).unwrap();

    assert!(backend.load_conversation_metadata(&id).is_err());
}

#[test]
fn remove_nonexistent_is_ok() {
    let backend = InMemoryStorageBackend::new();
    let id = test_id(1_000_000);
    backend.remove(&id).unwrap();
}

#[test]
fn load_ids_empty() {
    let backend = InMemoryStorageBackend::new();
    assert!(
        backend
            .load_conversation_ids(ConversationFilter::default())
            .is_empty()
    );
}

#[test]
fn load_ids_sorted() {
    let backend = InMemoryStorageBackend::new();
    let id1 = test_id(1_000_000);
    let id2 = test_id(2_000_000);

    // Insert in reverse order.
    backend
        .write(
            &id2,
            &Conversation::default(),
            &ConversationStream::new_test(),
        )
        .unwrap();
    backend
        .write(
            &id1,
            &Conversation::default(),
            &ConversationStream::new_test(),
        )
        .unwrap();

    let ids = backend.load_conversation_ids(ConversationFilter::default());
    assert_eq!(ids.len(), 2);
    assert!(ids[0] < ids[1], "IDs should be sorted");
}

#[test]
fn load_missing_metadata_errors() {
    let backend = InMemoryStorageBackend::new();
    let id = test_id(1_000_000);

    let err = backend.load_conversation_metadata(&id).unwrap_err();
    assert!(err.kind().is_missing());
}

#[test]
fn load_missing_stream_errors() {
    let backend = InMemoryStorageBackend::new();
    let id = test_id(1_000_000);

    let err = backend.load_conversation_stream(&id).unwrap_err();
    assert!(err.kind().is_missing());
}

#[test]
fn load_expired_none_when_no_expiry() {
    let backend = InMemoryStorageBackend::new();
    let id = test_id(1_000_000);

    backend
        .write(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
        )
        .unwrap();

    let expired = backend.load_expired_conversation_ids(Utc::now());
    assert!(expired.is_empty());
}

#[test]
fn load_expired_returns_past_conversations() {
    let backend = InMemoryStorageBackend::new();
    let id = test_id(1_000_000);

    let past = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let meta = Conversation::default().with_ephemeral(Some(past));
    backend
        .write(&id, &meta, &ConversationStream::new_test())
        .unwrap();

    let expired = backend.load_expired_conversation_ids(Utc::now());
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0], id);
}

#[test]
fn load_expired_skips_future_conversations() {
    let backend = InMemoryStorageBackend::new();
    let id = test_id(1_000_000);

    let future = Utc::now() + chrono::Duration::hours(1);
    let meta = Conversation::default().with_ephemeral(Some(future));
    backend
        .write(&id, &meta, &ConversationStream::new_test())
        .unwrap();

    let expired = backend.load_expired_conversation_ids(Utc::now());
    assert!(expired.is_empty());
}

#[test]
fn sanitize_returns_empty_report() {
    let backend = InMemoryStorageBackend::new();
    let report = backend.sanitize().unwrap();
    assert!(!report.has_repairs());
}

#[test]
fn lock_acquire_and_release() {
    let backend = InMemoryStorageBackend::new();

    let guard = backend.try_lock("conv-1", None).unwrap();
    assert!(guard.is_some(), "first lock should succeed");

    // Second attempt should fail.
    let second = backend.try_lock("conv-1", None).unwrap();
    assert!(second.is_none(), "second lock should fail");

    // Drop the guard.
    drop(guard);

    // Now it should succeed again.
    let third = backend.try_lock("conv-1", None).unwrap();
    assert!(third.is_some(), "lock after release should succeed");
}

#[test]
fn lock_different_conversations_independent() {
    let backend = InMemoryStorageBackend::new();

    let _g1 = backend.try_lock("conv-1", None).unwrap();
    let g2 = backend.try_lock("conv-2", None).unwrap();
    assert!(g2.is_some(), "different conversations should not conflict");
}

#[test]
fn lock_info_returns_none() {
    let backend = InMemoryStorageBackend::new();
    assert!(backend.lock_info("conv-1").is_none());
}

#[test]
fn list_orphaned_locks_empty() {
    let backend = InMemoryStorageBackend::new();
    assert!(backend.list_orphaned_locks().is_empty());
}

#[test]
fn session_roundtrip() {
    let backend = InMemoryStorageBackend::new();

    let data = serde_json::json!({ "value": "hello" });

    backend.save_session("sess-1", &data).unwrap();
    let loaded = backend.load_session("sess-1").unwrap().unwrap();
    assert_eq!(loaded, data);
}

#[test]
fn session_load_missing_returns_none() {
    let backend = InMemoryStorageBackend::new();
    let loaded = backend.load_session("nonexistent").unwrap();
    assert!(loaded.is_none());
}

#[test]
fn session_list_keys() {
    let backend = InMemoryStorageBackend::new();
    backend
        .save_session("a", &serde_json::json!("val-a"))
        .unwrap();
    backend
        .save_session("b", &serde_json::json!("val-b"))
        .unwrap();

    let mut keys = backend.list_session_keys();
    keys.sort();
    assert_eq!(keys, vec!["a", "b"]);
}

#[test]
fn session_overwrite() {
    let backend = InMemoryStorageBackend::new();

    backend
        .save_session("key", &serde_json::json!("first"))
        .unwrap();
    backend
        .save_session("key", &serde_json::json!("second"))
        .unwrap();

    let loaded = backend.load_session("key").unwrap().unwrap();
    assert_eq!(loaded, serde_json::json!("second"));
}
