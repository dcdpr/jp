use std::sync::Arc;

use camino_tempfile::{Utf8TempDir, tempdir};
use datetime_literal::datetime;
use jp_conversation::ConversationId;
use jp_storage::backend::{FsStorageBackend, LockBackend, SessionBackend};
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
            .with_user_storage(&user_root, None, "abc")
            .unwrap(),
    );
    let mut ws = Workspace::new(tmp.path()).with_backend(fs.clone());
    ws.disable_persistence();

    (tmp, ws, Some(fs))
}

#[test]
fn activate_session_conversation_bumps_last_activated_at_and_session() {
    let (_tmp, mut ws, _fs) = setup();
    let session = test_session();
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let original = datetime!(2025-07-19 14:00:00 Z);
    let now = datetime!(2025-07-19 16:30:00 Z);

    let config = Arc::new(jp_config::AppConfig::new_test());
    let conv = jp_conversation::Conversation {
        last_activated_at: original,
        ..Default::default()
    };
    ws.create_conversation_with_id(id, conv, config);

    let handle = ws.acquire_conversation(&id).unwrap();
    let lock = ws.test_lock(handle);

    ws.activate_session_conversation(&lock, &session, now)
        .unwrap();

    // Session mapping was recorded.
    assert_eq!(ws.session_active_conversation(&session), Some(id));

    // Conversation's last_activated_at was bumped to `now`. This is the
    // bug fix: previously, only the session mapping was written.
    let handle = ws.acquire_conversation(&id).unwrap();
    let meta = ws.metadata(&handle).unwrap();
    assert_eq!(meta.last_activated_at, now);
}

#[test]
fn record_session_activation_does_not_touch_conversation_metadata() {
    let (_tmp, mut ws, _fs) = setup();
    let session = test_session();
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let original = datetime!(2025-07-19 14:00:00 Z);
    let now = datetime!(2025-07-19 16:30:00 Z);

    let config = Arc::new(jp_config::AppConfig::new_test());
    let conv = jp_conversation::Conversation {
        last_activated_at: original,
        ..Default::default()
    };
    ws.create_conversation_with_id(id, conv, config);

    ws.record_session_activation(&session, id, now).unwrap();

    // Session mapping was recorded.
    assert_eq!(ws.session_active_conversation(&session), Some(id));

    // The bare form must NOT change conversation metadata — it's used in
    // the lock-contention fallback (`jp c use` while another process holds
    // the lock) and in tests that exercise session-mapping logic.
    let handle = ws.acquire_conversation(&id).unwrap();
    let meta = ws.metadata(&handle).unwrap();
    assert_eq!(
        meta.last_activated_at, original,
        "record_session_activation must leave last_activated_at untouched"
    );
}

#[test]
fn activate_creates_new_mapping() {
    let (_tmp, mut ws, _fs) = setup();
    let session = test_session();
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let now = datetime!(2025-07-19 14:30:00 Z);

    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    ws.record_session_activation(&session, id, now).unwrap();

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

    ws.record_session_activation(&session, id1, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.record_session_activation(&session, id2, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();
    // Re-activate id1 — should move it to the front, not duplicate.
    ws.record_session_activation(&session, id1, datetime!(2025-07-19 16:00:00 Z))
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
    let mut mapping = SessionMapping::new(SessionId::new("12345").unwrap(), SessionSource::Getsid);
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

    ws.record_session_activation(&session, id, datetime!(2025-07-19 14:00:00 Z))
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
        ws.record_session_activation(&session, id, datetime!(2025-07-19 14:00:00 Z))
            .is_err()
    );
}

#[test]
fn env_source_liveness_is_unknown() {
    let id = SessionId::new("anything").unwrap();
    let source = SessionSource::env("JP_SESSION");
    assert!(matches!(
        is_session_process_liveness(&id, &source),
        Liveness::Unknown
    ));
}

/// The platform handle survives the id encode/decode round-trip.
/// This is the regression guard for the encode-as-A / decode-as-B bug: it runs
/// on every platform's CI, not only the one whose syscall path is exercised.
#[test]
fn getsid_id_roundtrips_to_pid() {
    assert_eq!(Session::getsid(12345).id.as_pid(), Some(12345));
}

#[test]
fn hwnd_id_roundtrips_to_handle() {
    assert_eq!(Session::hwnd(0xBEEF).id.as_hwnd(), Some(0xBEEF));
}

#[test]
fn non_numeric_id_does_not_decode_to_a_handle() {
    let id = SessionId::new("not-a-handle").unwrap();
    assert_eq!(id.as_pid(), None);
    assert_eq!(id.as_hwnd(), None);
    assert!(matches!(
        is_session_process_liveness(&id, &SessionSource::Getsid),
        Liveness::Unknown
    ));
}

#[cfg(unix)]
#[test]
fn getsid_with_own_pid_is_alive() {
    let pid = std::process::id().cast_signed();
    assert!(matches!(pid_liveness(pid), Liveness::Alive));
}

#[cfg(unix)]
#[test]
fn getsid_with_nonexistent_pid_is_dead() {
    // PID 2_000_000_000 is extremely unlikely to exist.
    assert!(matches!(pid_liveness(2_000_000_000), Liveness::Dead));
}

#[cfg(windows)]
#[test]
fn hwnd_with_own_console_is_alive() {
    // GetConsoleWindow returns the HWND for the current console.
    // It should be reported as alive.
    let hwnd = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
    if !hwnd.is_null() {
        // Round-trip through the real encode (Session::hwnd) and decode
        // (SessionId::as_hwnd) so a format mismatch fails here, not silently.
        let session = Session::hwnd(hwnd as isize);
        let handle = session.id.as_hwnd().expect("hwnd id must decode");
        assert!(matches!(hwnd_liveness(handle), Liveness::Alive));
    }
    // If hwnd is null (no console, e.g. GUI-only CI), skip silently.
}

#[cfg(windows)]
#[test]
fn hwnd_with_nonexistent_handle_is_dead() {
    // 0xDEAD is extremely unlikely to be a valid window handle.
    assert!(matches!(hwnd_liveness(0xDEAD), Liveness::Dead));
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
    ws.record_session_activation(&session, id, datetime!(2025-07-19 14:00:00 Z))
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
    ws.record_session_activation(&session, ghost_id, datetime!(2025-07-19 18:00:00 Z))
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

    ws.record_session_activation(&session, id, datetime!(2025-07-19 14:00:00 Z))
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

    ws.record_session_activation(&session_a, id1, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.record_session_activation(&session_b, id2, datetime!(2025-07-19 15:00:00 Z))
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
            .with_user_storage(&user_root, None, "abc")
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
    ws.record_session_activation(&session_b, conv_other, datetime!(2025-07-19 16:00:00 Z))
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
            .with_user_storage(&user_root, None, "abc")
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

    ws.record_session_activation(&session, id, datetime!(2025-07-19 14:00:00 Z))
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
    ws.record_session_activation(&session, id, datetime!(2025-07-19 14:00:00 Z))
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

    ws.record_session_activation(&session, id1, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.record_session_activation(&session, id2, datetime!(2025-07-19 15:00:00 Z))
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
            .with_user_storage(&user_root, None, "abc")
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

    ws.record_session_activation(&session, dead_id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.record_session_activation(&session, locked_id, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();
    ws.record_session_activation(&session, live_id, datetime!(2025-07-19 16:00:00 Z))
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
            .with_user_storage(&user_root, None, "abc")
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
    ws.record_session_activation(&session, dead_id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.record_session_activation(&session, live_id, datetime!(2025-07-19 15:00:00 Z))
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

#[test]
fn storage_key_distinguishes_env_vars_with_same_value() {
    let jp = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let tmux = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::env("TMUX_PANE"),
    };

    assert_ne!(jp.storage_key(), tmux.storage_key());
}

#[test]
fn storage_key_distinguishes_getsid_from_env_with_same_value() {
    let getsid = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::Getsid,
    };
    let env = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };

    assert_eq!(getsid.storage_key(), "getsid-1234");
    assert_ne!(getsid.storage_key(), env.storage_key());
}

/// Regression test for the bug this fix resolves: two env-sourced sessions with
/// the same value but different variables used to collide on one file.
#[test]
fn env_sessions_with_same_value_do_not_collide() {
    let (_tmp, mut ws, _fs) = setup();
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());

    let jp = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let tmux = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::env("TMUX_PANE"),
    };

    let id1 = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2025-07-19 15:00:00 Z)).unwrap();
    ws.create_conversation_with_id(
        id1,
        jp_conversation::Conversation::default(),
        config.clone(),
    );
    ws.create_conversation_with_id(id2, jp_conversation::Conversation::default(), config);

    ws.record_session_activation(&jp, id1, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.record_session_activation(&tmux, id2, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();

    // Each session keeps its own active conversation; no overwrite.
    assert_eq!(ws.session_active_conversation(&jp), Some(id1));
    assert_eq!(ws.session_active_conversation(&tmux), Some(id2));
}

/// A pre-fix file keyed on the bare value (no `id`, no source prefix) is still
/// readable, and a subsequent write updates that same file rather than forking
/// a duplicate.
#[test]
fn legacy_bare_value_file_is_read_via_fallback() {
    let (_tmp, mut ws, fs) = setup();
    let fs = fs.unwrap();
    let session = Session {
        id: SessionId::new("99887").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    // Write a legacy-format mapping: keyed on the bare value, no `id` field.
    let legacy = serde_json::json!({
        "history": [{ "id": id, "activated_at": "2025-07-19T14:00:00Z" }],
        "source": { "type": "env", "key": "JP_SESSION" }
    });
    fs.save_session("99887", &legacy).unwrap();

    // Resolves via the bare-value fallback even though storage_key is prefixed.
    assert_eq!(ws.session_active_conversation(&session), Some(id));

    // A write lands on the existing legacy file, so there is still one file.
    ws.record_session_activation(&session, id, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();
    assert_eq!(fs.list_session_files().len(), 1);
}

/// Cleanup migrates a surviving legacy file to its source-prefixed name and
/// removes the old file.
#[test]
fn cleanup_migrates_legacy_filename_to_source_prefixed_key() {
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
        id: SessionId::new("ci-123").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    // Conversation on disk so the Env existence heuristic keeps the mapping.
    fs.write_test_conversation(&id, &jp_conversation::Conversation::default());

    let legacy = serde_json::json!({
        "history": [{ "id": id, "activated_at": "2025-07-19T14:00:00Z" }],
        "source": { "type": "env", "key": "JP_SESSION" }
    });
    fs.save_session("ci-123", &legacy).unwrap();

    ws.cleanup_stale_files(Some(&fs));

    let files = fs.list_session_files();
    assert_eq!(files.len(), 1, "legacy file migrated, not duplicated");
    assert_eq!(files[0].file_stem().unwrap(), session.storage_key());
    // The conversation is on disk but not in the in-memory index, so verify via
    // the raw mapping (which the migrated file still resolves to).
    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.active_conversation_id(), Some(id));
}

/// A legacy bare-value file written by one source must not be adopted by a
/// different source that happens to share the value — otherwise the pre-fix
/// collision survives the upgrade for the first mismatched session.
#[test]
fn legacy_fallback_ignores_mapping_with_mismatched_source() {
    let (_tmp, mut ws, fs) = setup();
    let fs = fs.unwrap();
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    // Pre-upgrade collision file: keyed on the bare value `1234`, source
    // `JP_SESSION`.
    let legacy = serde_json::json!({
        "history": [{ "id": id, "activated_at": "2025-07-19T14:00:00Z" }],
        "source": { "type": "env", "key": "JP_SESSION" }
    });
    fs.save_session("1234", &legacy).unwrap();

    // A different env var with the same value must NOT inherit that history.
    let tmux = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::env("TMUX_PANE"),
    };
    assert_eq!(ws.session_active_conversation(&tmux), None);

    // The matching source still resolves the legacy file via the fallback.
    let jp = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    assert_eq!(ws.session_active_conversation(&jp), Some(id));
}

#[test]
fn storage_key_neutralizes_path_traversal_in_env_key() {
    let session = Session {
        id: SessionId::new("1234").unwrap(),
        source: SessionSource::env("../../etc/passwd"),
    };

    let key = session.storage_key();
    assert!(
        !key.contains('/'),
        "key must not contain path separators: {key}"
    );
    assert!(!key.contains(".."), "key must not contain `..`: {key}");
}

#[test]
fn storage_key_neutralizes_path_traversal_in_getsid_id() {
    // A tampered or externally constructed mapping can carry an arbitrary id;
    // the Getsid/Hwnd arms interpolate it, so it must be sanitized too.
    let session = Session {
        id: SessionId::new("x/../../outside").unwrap(),
        source: SessionSource::Getsid,
    };

    let key = session.storage_key();
    assert!(
        !key.contains('/'),
        "key must not contain path separators: {key}"
    );
    assert!(!key.contains(".."), "key must not contain `..`: {key}");
}

/// `storage_key` is not injective — two distinct env sources can sanitize to
/// the same key.
/// The primary read must reject a mapping whose stored source does not match,
/// just like the legacy fallback does.
#[test]
fn primary_read_ignores_mapping_from_aliased_source() {
    let (_tmp, mut ws, _fs) = setup();
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    // `A_B` and `A-B` sanitize to the same key segment with the same value.
    let stored = Session {
        id: SessionId::new("v").unwrap(),
        source: SessionSource::env("A_B"),
    };
    let alias = Session {
        id: SessionId::new("v").unwrap(),
        source: SessionSource::env("A-B"),
    };
    assert_eq!(
        stored.storage_key(),
        alias.storage_key(),
        "precondition: the two sources alias to one key"
    );

    ws.record_session_activation(&stored, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();

    // The aliasing source must not adopt the stored session's history.
    assert_eq!(ws.session_active_conversation(&alias), None);
    // The owning source still resolves it.
    assert_eq!(ws.session_active_conversation(&stored), Some(id));
}

/// An env *value* containing path components must not escape `sessions/` on the
/// legacy compat probe (read) or on the subsequent write.
#[test]
fn unsafe_env_value_does_not_escape_on_read_or_write() {
    let (_tmp, mut ws, fs) = setup();
    let fs = fs.unwrap();
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    let session = Session {
        id: SessionId::new("x/../../outside").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };

    // The legacy probe is skipped for the unsafe value, so this resolves cleanly
    // to None rather than statting an escaped path.
    assert_eq!(ws.session_active_conversation(&session), None);

    // The write lands on the hashed, source-prefixed key: a single safe segment.
    ws.record_session_activation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    let files = fs.list_session_files();
    assert_eq!(files.len(), 1);
    let stem = files[0].file_stem().unwrap();
    assert!(
        !stem.contains('/') && !stem.contains(".."),
        "unsafe stem: {stem}"
    );
}
