use std::sync::Arc;

use camino_tempfile::{Utf8TempDir, tempdir};
use datetime_literal::datetime;
use jp_conversation::ConversationId;
use jp_storage::backend::{FsStorageBackend, LockBackend};
use test_log::test;

use super::*;
use crate::{
    Workspace,
    session::{Session, SessionId, SessionSource},
};

fn test_session() -> Session {
    Session {
        id: SessionId::new("12345").unwrap(),
        source: SessionSource::Getsid,
    }
}

/// Create a workspace with both workspace and user storage configured.
fn setup() -> (Utf8TempDir, Workspace, Option<Arc<FsStorageBackend>>) {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");

    let fs = Arc::new(
        FsStorageBackend::new(&storage_path)
            .unwrap()
            .with_user_storage(&user_root, "test-ws", "abc")
            .unwrap(),
    );
    let mut ws = Workspace::new(tmp.path()).with_backend(fs.clone());
    ws.disable_persistence();

    (tmp, ws, Some(fs))
}

#[test]
fn activate_creates_new_mapping() {
    let (_tmp, mut ws, _fs) = setup();
    let session = test_session();
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let now = datetime!(2025-07-19 14:30:00 Z);

    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    ws.activate_session_conversation(&session, id, now).unwrap();

    assert_eq!(ws.session_active_conversation(&session), Some(id));
    assert_eq!(ws.session_previous_conversation(&session), None);
}

#[test]
fn activate_deduplicates_history() {
    let (_tmp, mut ws, _fs) = setup();
    let session = test_session();
    let id1 = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2025-07-19 15:00:00 Z)).unwrap();

    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(
        id1,
        jp_conversation::Conversation::default(),
        config.clone(),
    );
    ws.create_conversation_with_id(id2, jp_conversation::Conversation::default(), config);

    ws.activate_session_conversation(&session, id1, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.activate_session_conversation(&session, id2, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();
    // Re-activate id1 — should move it to the front, not duplicate.
    ws.activate_session_conversation(&session, id1, datetime!(2025-07-19 16:00:00 Z))
        .unwrap();

    assert_eq!(ws.session_active_conversation(&session), Some(id1));
    assert_eq!(ws.session_previous_conversation(&session), Some(id2));

    // Verify dedup by reloading from disk.
    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.history.len(), 2);
    assert_eq!(
        mapping.history[0].activated_at,
        datetime!(2025-07-19 16:00:00 Z)
    );
}

#[test]
fn load_returns_none_when_missing() {
    let (_tmp, ws, _fs) = setup();
    let session = test_session();

    assert!(ws.session_active_conversation(&session).is_none());
}

#[test]
fn previous_conversation_id_with_single_entry() {
    let mut mapping = SessionMapping::new(SessionSource::Getsid);
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    mapping.activate(id, datetime!(2025-07-19 14:00:00 Z));

    assert_eq!(mapping.active_conversation_id(), Some(id));
    assert_eq!(mapping.previous_conversation_id(), None);
}

#[test]
fn env_source_roundtrips_through_json() {
    let (_tmp, ws, _fs) = setup();
    let session = Session {
        id: SessionId::new("my-session").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();

    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.source, SessionSource::Env {
        key: "JP_SESSION".to_owned()
    });
    assert_eq!(mapping.active_conversation_id(), Some(id));
}

#[test]
fn no_user_storage_returns_none() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");

    // Workspace without user storage.
    let fs = Arc::new(FsStorageBackend::new(&storage_path).unwrap());
    let mut ws = Workspace::new(tmp.path()).with_backend(fs);
    ws.disable_persistence();

    let session = test_session();
    assert!(ws.session_active_conversation(&session).is_none());
}

#[test]
fn no_user_storage_returns_error_on_write() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");

    let fs = Arc::new(FsStorageBackend::new(&storage_path).unwrap());
    let mut ws = Workspace::new(tmp.path()).with_backend(fs);
    ws.disable_persistence();

    let session = test_session();
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    assert!(
        ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
            .is_err()
    );
}

#[test]
fn env_source_liveness_is_unknown() {
    let source = SessionSource::env("JP_SESSION");
    assert!(matches!(
        is_session_process_liveness(&source, "anything"),
        Liveness::Unknown
    ));
}

#[cfg(unix)]
#[test]
fn getsid_with_own_pid_is_alive() {
    let pid = std::process::id().to_string();
    assert!(matches!(pid_liveness(&pid), Liveness::Alive));
}

#[cfg(unix)]
#[test]
fn getsid_with_nonexistent_pid_is_dead() {
    // PID 2_000_000_000 is extremely unlikely to exist.
    assert!(matches!(pid_liveness("2000000000"), Liveness::Dead));
}

#[cfg(unix)]
#[test]
fn getsid_with_unparseable_key_is_unknown() {
    assert!(matches!(pid_liveness("not-a-pid"), Liveness::Unknown));
}

#[cfg(windows)]
#[test]
fn hwnd_with_own_console_is_alive() {
    // GetConsoleWindow returns the HWND for the current console.
    // It should be reported as alive.
    let hwnd = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
    if !hwnd.is_null() {
        let key = format!("{}", hwnd as isize);
        assert!(matches!(hwnd_liveness(&key), Liveness::Alive));
    }
    // If hwnd is null (no console, e.g. GUI-only CI), skip silently.
}

#[cfg(windows)]
#[test]
fn hwnd_with_nonexistent_handle_is_dead() {
    // 0xDEAD is extremely unlikely to be a valid window handle.
    assert!(matches!(hwnd_liveness("57005"), Liveness::Dead));
}

#[cfg(windows)]
#[test]
fn hwnd_with_unparseable_key_is_unknown() {
    assert!(matches!(hwnd_liveness("not-a-handle"), Liveness::Unknown));
}

#[cfg(unix)]
#[test]
fn cleanup_removes_stale_getsid_session() {
    let (_tmp, mut ws, fs) = setup();
    let session = Session {
        id: SessionId::new("2000000000").unwrap(), // nonexistent PID
        source: SessionSource::Getsid,
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    // Create a conversation so the workspace has content.
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    // Write a session mapping referencing a live conversation.
    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    assert!(ws.session_active_conversation(&session).is_some());

    // Cleanup should remove it because PID 2000000000 is dead,
    // even though the conversation still exists.
    ws.cleanup_stale_files(fs.as_deref());

    assert!(
        ws.session_active_conversation(&session).is_none(),
        "Stale getsid session should be cleaned up when process is dead"
    );
}

#[cfg(unix)]
#[test]
fn cleanup_keeps_getsid_session_file_but_prunes_ghost_entries() {
    let (_tmp, ws, fs) = setup();

    // Use our own PID as the session key — guaranteed to be alive.
    let own_pid = std::process::id().to_string();
    let session = Session {
        id: SessionId::new(&own_pid).unwrap(),
        source: SessionSource::Getsid,
    };

    // Reference a conversation that does NOT exist on disk or in memory,
    // and has no active lock. The session file should survive (alive
    // process), but the ghost entry should be pruned (unlocked + absent).
    let ghost_id = ConversationId::try_from(datetime!(2025-07-19 18:00:00 Z)).unwrap();
    ws.activate_session_conversation(&session, ghost_id, datetime!(2025-07-19 18:00:00 Z))
        .unwrap();

    ws.cleanup_stale_files(fs.as_deref());

    // Session file survives (process is alive).
    let mapping = ws.load_session_mapping(&session);
    assert!(
        mapping.is_some(),
        "Getsid session file must survive cleanup when process is alive"
    );
    // But the ghost entry was pruned (not on disk, not locked).
    assert!(
        mapping.unwrap().history.is_empty(),
        "Ghost entry should be pruned when conversation is absent and unlocked"
    );
}

#[cfg(windows)]
#[test]
fn cleanup_removes_stale_hwnd_session() {
    let (_tmp, mut ws, fs) = setup();
    let session = Session {
        id: SessionId::new("57005").unwrap(), // 0xDEAD — unlikely to be a valid HWND
        source: SessionSource::Hwnd,
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    assert!(ws.session_active_conversation(&session).is_some());

    // Cleanup should remove it because HWND 0xDEAD is not a valid window.
    ws.cleanup_stale_files(fs.as_deref());

    assert!(
        ws.session_active_conversation(&session).is_none(),
        "Stale hwnd session should be cleaned up when window handle is dead"
    );
}

#[test]
fn all_active_conversation_ids_across_sessions() {
    let (_tmp, mut ws, _fs) = setup();
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());

    let session_a = Session {
        id: SessionId::new("sess-a").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let session_b = Session {
        id: SessionId::new("sess-b").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };

    let id1 = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2025-07-19 15:00:00 Z)).unwrap();

    ws.create_conversation_with_id(
        id1,
        jp_conversation::Conversation::default(),
        config.clone(),
    );
    ws.create_conversation_with_id(id2, jp_conversation::Conversation::default(), config);

    ws.activate_session_conversation(&session_a, id1, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.activate_session_conversation(&session_b, id2, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();

    let mut active = ws.all_active_conversation_ids();
    active.sort();

    let mut expected = vec![id1, id2];
    expected.sort();

    assert_eq!(active, expected);
}

#[test]
fn all_active_conversation_ids_empty_when_no_sessions() {
    let (_tmp, ws, _fs) = setup();
    assert!(ws.all_active_conversation_ids().is_empty());
}

#[test]
fn cleanup_keeps_session_referencing_conversation_created_after_index_load() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");

    let fs = Arc::new(
        FsStorageBackend::new(&storage_path)
            .unwrap()
            .with_user_storage(&user_root, "test-ws", "abc")
            .unwrap(),
    );
    let mut ws = Workspace::new(tmp.path()).with_backend(fs.clone());
    ws.disable_persistence();
    let fs = Some(fs);

    // Session B references a conversation that exists on disk but was NOT in
    // the workspace's in-memory index (simulates a conversation created by
    // another process after our load_conversation_index call).
    let session_b = Session {
        id: SessionId::new("sess-other-tab").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let conv_other = ConversationId::try_from(datetime!(2025-07-19 16:00:00 Z)).unwrap();

    // Write the conversation to disk directly, bypassing the in-memory state.
    // This simulates another `jp` process persisting a conversation.
    fs.as_ref()
        .unwrap()
        .write_test_conversation(&conv_other, &jp_conversation::Conversation::default());

    // Write a session mapping pointing at that conversation.
    ws.activate_session_conversation(&session_b, conv_other, datetime!(2025-07-19 16:00:00 Z))
        .unwrap();

    // Verify precondition: the conversation is NOT in the in-memory index.
    assert!(
        !ws.conversations().any(|(id, _)| *id == conv_other),
        "Conversation should not be in the in-memory index"
    );

    // Cleanup must NOT delete session_b — the conversation exists on disk.
    ws.cleanup_stale_files(fs.as_deref());

    // The conversation isn't in the in-memory index, so
    // session_active_conversation correctly returns None. Verify the
    // session file survives via load_session_mapping.
    let mapping = ws.load_session_mapping(&session_b);
    assert!(
        mapping.is_some(),
        "Session referencing a conversation created by another process should survive cleanup"
    );
    assert_eq!(mapping.unwrap().active_conversation_id(), Some(conv_other),);
}

#[test]
fn cleanup_keeps_env_session_with_live_conversations() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");

    let fs = Arc::new(
        FsStorageBackend::new(&storage_path)
            .unwrap()
            .with_user_storage(&user_root, "test-ws", "abc")
            .unwrap(),
    );
    let mut ws = Workspace::new(tmp.path()).with_backend(fs.clone());
    ws.disable_persistence();
    let fs = Some(fs);

    let session = Session {
        id: SessionId::new("my-ci-session").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    // Write conversation to disk so the disk-based cleanup scan finds it.
    fs.as_ref()
        .unwrap()
        .write_test_conversation(&id, &jp_conversation::Conversation::default());

    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();

    ws.cleanup_stale_files(fs.as_deref());

    // The conversation isn't in the in-memory index, so
    // session_active_conversation correctly returns None. Verify the
    // session file survives via load_session_mapping.
    let mapping = ws.load_session_mapping(&session);
    assert!(
        mapping.is_some(),
        "Env session with live conversations should not be cleaned up"
    );
    assert_eq!(mapping.unwrap().active_conversation_id(), Some(id));
}

#[test]
fn active_conversation_returns_none_for_deleted_conversation() {
    let (_tmp, mut ws, _fs) = setup();
    let session = test_session();
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());

    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);
    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();

    assert_eq!(ws.session_active_conversation(&session), Some(id));

    // Remove the conversation from the in-memory index to simulate deletion.
    ws.state.conversations.remove(&id);

    assert_eq!(
        ws.session_active_conversation(&session),
        None,
        "Must return None when the referenced conversation no longer exists"
    );

    // The raw mapping still has the entry.
    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.active_conversation_id(), Some(id));
}

#[test]
fn previous_conversation_returns_none_for_deleted_conversation() {
    let (_tmp, mut ws, _fs) = setup();
    let session = test_session();
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());

    let id1 = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2025-07-19 15:00:00 Z)).unwrap();
    ws.create_conversation_with_id(
        id1,
        jp_conversation::Conversation::default(),
        config.clone(),
    );
    ws.create_conversation_with_id(id2, jp_conversation::Conversation::default(), config);

    ws.activate_session_conversation(&session, id1, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.activate_session_conversation(&session, id2, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();

    assert_eq!(ws.session_previous_conversation(&session), Some(id1));

    // Remove id1 to simulate deletion.
    ws.state.conversations.remove(&id1);

    assert_eq!(
        ws.session_previous_conversation(&session),
        None,
        "Must return None when the previous conversation no longer exists"
    );
}

#[test]
fn cleanup_skips_pruning_locked_conversations() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");

    let fs = Arc::new(
        FsStorageBackend::new(&storage_path)
            .unwrap()
            .with_user_storage(&user_root, "test-ws", "abc")
            .unwrap(),
    );
    let mut ws = Workspace::new(tmp.path()).with_backend(fs.clone());
    ws.disable_persistence();

    let session = Session {
        id: SessionId::new("lock-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };

    let live_id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let locked_id = ConversationId::try_from(datetime!(2025-07-19 15:00:00 Z)).unwrap();
    let dead_id = ConversationId::try_from(datetime!(2025-07-19 16:00:00 Z)).unwrap();

    // live_id exists on disk, locked_id is absent but locked, dead_id is gone.
    fs.write_test_conversation(&live_id, &jp_conversation::Conversation::default());
    let _lock = fs
        .try_lock(&locked_id.to_string(), None)
        .unwrap()
        .expect("should acquire lock");

    ws.activate_session_conversation(&session, dead_id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.activate_session_conversation(&session, locked_id, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();
    ws.activate_session_conversation(&session, live_id, datetime!(2025-07-19 16:00:00 Z))
        .unwrap();

    // Before cleanup: 3 entries.
    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.history.len(), 3);

    ws.cleanup_stale_files(Some(&fs));

    // dead_id pruned, locked_id kept (lock held), live_id kept (on disk).
    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.history.len(), 2);
    assert_eq!(mapping.history[0].id, live_id);
    assert_eq!(mapping.history[1].id, locked_id);
}

#[test]
fn cleanup_prunes_dead_entries_from_session_history() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");

    let fs = Arc::new(
        FsStorageBackend::new(&storage_path)
            .unwrap()
            .with_user_storage(&user_root, "test-ws", "abc")
            .unwrap(),
    );
    let mut ws = Workspace::new(tmp.path()).with_backend(fs.clone());
    ws.disable_persistence();

    let session = Session {
        id: SessionId::new("env-session").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };

    let live_id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let dead_id = ConversationId::try_from(datetime!(2025-07-19 15:00:00 Z)).unwrap();

    // Only write the live conversation to disk.
    fs.write_test_conversation(&live_id, &jp_conversation::Conversation::default());

    // Activate both: dead first, then live (so live is history[0]).
    ws.activate_session_conversation(&session, dead_id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.activate_session_conversation(&session, live_id, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();

    // Before cleanup: both entries in history.
    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.history.len(), 2);

    ws.cleanup_stale_files(Some(&fs));

    // After cleanup: dead entry pruned, live entry remains.
    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.history.len(), 1);
    assert_eq!(mapping.active_conversation_id(), Some(live_id));
}
