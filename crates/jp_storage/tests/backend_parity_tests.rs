//! Parity tests: verify that `FsStorageBackend` and `InMemoryStorageBackend`
//! exhibit the same observable behavior across all four backend traits.
//!
//! Each test is run against both backends via the `with_backends!` helper.

use std::sync::Arc;

use camino_tempfile::tempdir;
use chrono::{TimeZone as _, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::backend::{
    FsStorageBackend, InMemoryStorageBackend, LoadBackend, LockBackend, NullLockBackend,
    PersistBackend, SessionBackend,
};

fn test_id(secs: i64) -> ConversationId {
    ConversationId::try_from(Utc.timestamp_opt(secs, 0).unwrap()).unwrap()
}

/// Run a test body against both an `FsStorageBackend` and an
/// `InMemoryStorageBackend`.
macro_rules! with_backends {
    ($name:ident, |$b:ident| $body:block) => {
        mod $name {
            use super::*;

            #[test]
            fn fs() {
                let dir = tempdir().unwrap();
                let $b: Arc<dyn BackendBundle> = Arc::new(
                    FsStorageBackend::new(dir.path())
                        .unwrap()
                        .with_user_storage(dir.path(), "test", "parity")
                        .unwrap(),
                );
                $body
            }

            #[test]
            fn memory() {
                let $b: Arc<dyn BackendBundle> = Arc::new(InMemoryStorageBackend::new());
                $body
            }
        }
    };
}

/// Trait alias so tests can use a single backend object for all four traits.
trait BackendBundle: PersistBackend + LoadBackend + LockBackend + SessionBackend {}
impl<T: PersistBackend + LoadBackend + LockBackend + SessionBackend> BackendBundle for T {}

with_backends!(write_then_load_metadata, |b| {
    let id = test_id(1_000_000);
    let meta = Conversation {
        title: Some("hello".into()),
        ..Default::default()
    };
    let events = ConversationStream::new_test();

    b.write(&id, &meta, &events).unwrap();

    let loaded = b.load_conversation_metadata(&id).unwrap();
    assert_eq!(loaded.title.as_deref(), Some("hello"));
});

with_backends!(write_then_load_stream, |b| {
    let id = test_id(1_000_000);
    let meta = Conversation::default();
    let events = ConversationStream::new_test();

    b.write(&id, &meta, &events).unwrap();

    let loaded = b.load_conversation_stream(&id).unwrap();
    assert!(loaded.is_empty());
});

with_backends!(remove_then_load_fails, |b| {
    let id = test_id(1_000_000);
    b.write(
        &id,
        &Conversation::default(),
        &ConversationStream::new_test(),
    )
    .unwrap();

    b.remove(&id).unwrap();

    assert!(b.load_conversation_metadata(&id).is_err());
    assert!(b.load_conversation_stream(&id).is_err());
});

with_backends!(remove_nonexistent_is_ok, |b| {
    let id = test_id(1_000_000);
    b.remove(&id).unwrap();
});

with_backends!(load_missing_metadata_errors, |b| {
    let id = test_id(1_000_000);
    assert!(b.load_conversation_metadata(&id).is_err());
});

with_backends!(load_missing_stream_errors, |b| {
    let id = test_id(1_000_000);
    assert!(b.load_conversation_stream(&id).is_err());
});

with_backends!(load_all_ids_empty, |b| {
    assert!(b.load_all_conversation_ids().is_empty());
});

with_backends!(load_all_ids_returns_written, |b| {
    let id1 = test_id(1_000_000);
    let id2 = test_id(2_000_000);

    b.write(
        &id2,
        &Conversation::default(),
        &ConversationStream::new_test(),
    )
    .unwrap();
    b.write(
        &id1,
        &Conversation::default(),
        &ConversationStream::new_test(),
    )
    .unwrap();

    let ids = b.load_all_conversation_ids();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));
});

with_backends!(load_expired_none_when_no_expiry, |b| {
    let id = test_id(1_000_000);
    b.write(
        &id,
        &Conversation::default(),
        &ConversationStream::new_test(),
    )
    .unwrap();

    let expired = b.load_expired_conversation_ids(Utc::now());
    assert!(expired.is_empty());
});

with_backends!(load_expired_returns_past, |b| {
    let id = test_id(1_000_000);
    let past = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let meta = Conversation::default().with_ephemeral(Some(past));

    b.write(&id, &meta, &ConversationStream::new_test())
        .unwrap();

    let expired = b.load_expired_conversation_ids(Utc::now());
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0], id);
});

with_backends!(load_expired_skips_future, |b| {
    let id = test_id(1_000_000);
    let future = Utc::now() + chrono::Duration::hours(1);
    let meta = Conversation::default().with_ephemeral(Some(future));

    b.write(&id, &meta, &ConversationStream::new_test())
        .unwrap();

    let expired = b.load_expired_conversation_ids(Utc::now());
    assert!(expired.is_empty());
});

with_backends!(sanitize_empty_store, |b| {
    let report = b.sanitize().unwrap();
    assert!(!report.has_repairs());
});

with_backends!(lock_acquire_and_release, |b| {
    let guard = b.try_lock("conv-1", None).unwrap();
    assert!(guard.is_some(), "first lock should succeed");

    // Second attempt on the same conversation.
    let second = b.try_lock("conv-1", None).unwrap();
    // Both backends should deny the second lock while the first is held.
    // (FsStorageBackend uses flock, InMemoryStorageBackend uses a HashSet.)
    assert!(second.is_none(), "second lock should fail");

    drop(guard);

    let third = b.try_lock("conv-1", None).unwrap();
    assert!(third.is_some(), "lock after release should succeed");
});

with_backends!(lock_different_conversations_independent, |b| {
    let _g1 = b.try_lock("conv-1", None).unwrap();
    let g2 = b.try_lock("conv-2", None).unwrap();
    assert!(g2.is_some(), "different conversations should not conflict");
});

with_backends!(session_roundtrip, |b| {
    let data = serde_json::json!({ "conv": "abc" });
    b.save_session("sess-1", &data).unwrap();

    let loaded = b.load_session("sess-1").unwrap().unwrap();
    assert_eq!(loaded, data);
});

with_backends!(session_load_missing_returns_none, |b| {
    let loaded = b.load_session("nonexistent").unwrap();
    assert!(loaded.is_none());
});

with_backends!(session_overwrite, |b| {
    b.save_session("key", &serde_json::json!("first")).unwrap();
    b.save_session("key", &serde_json::json!("second")).unwrap();

    let loaded = b.load_session("key").unwrap().unwrap();
    assert_eq!(loaded, serde_json::json!("second"));
});

with_backends!(session_list_keys, |b| {
    b.save_session("alpha", &serde_json::json!("a")).unwrap();
    b.save_session("beta", &serde_json::json!("b")).unwrap();

    let mut keys = b.list_session_keys();
    keys.sort();
    assert_eq!(keys, vec!["alpha", "beta"]);
});

#[test]
fn null_lock_always_succeeds() {
    let backend = NullLockBackend;

    let g1 = backend.try_lock("conv-1", None).unwrap();
    assert!(g1.is_some());

    // A second lock on the same conversation also succeeds (no exclusion).
    let g2 = backend.try_lock("conv-1", None).unwrap();
    assert!(g2.is_some());
}

#[test]
fn null_lock_info_returns_none() {
    let backend = NullLockBackend;
    assert!(backend.lock_info("conv-1").is_none());
}

#[test]
fn null_lock_no_orphans() {
    let backend = NullLockBackend;
    assert!(backend.list_orphaned_locks().is_empty());
}
