use camino_tempfile::{Utf8TempDir, tempdir};
use datetime_literal::datetime;
use jp_conversation::ConversationId;
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
fn setup() -> (Utf8TempDir, Workspace) {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");

    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap()
        .with_local_storage_at(&user_root, "test-ws", "abc")
        .unwrap();
    ws.disable_persistence();

    (tmp, ws)
}

#[test]
fn activate_creates_new_mapping() {
    let (_tmp, ws) = setup();
    let session = test_session();
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let now = datetime!(2025-07-19 14:30:00 Z);

    ws.activate_session_conversation(&session, id, now).unwrap();

    assert_eq!(ws.session_active_conversation(&session), Some(id));
    assert_eq!(ws.session_previous_conversation(&session), None);
}

#[test]
fn activate_deduplicates_history() {
    let (_tmp, ws) = setup();
    let session = test_session();
    let id1 = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2025-07-19 15:00:00 Z)).unwrap();

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
    let (_tmp, ws) = setup();
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
    let (_tmp, ws) = setup();
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
    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap();
    ws.disable_persistence();

    let session = test_session();
    assert!(ws.session_active_conversation(&session).is_none());
}

#[test]
fn no_user_storage_returns_error_on_write() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");

    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap();
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
    let (_tmp, mut ws) = setup();
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
    ws.cleanup_stale_files();

    assert!(
        ws.session_active_conversation(&session).is_none(),
        "Stale getsid session should be cleaned up when process is dead"
    );
}

#[cfg(unix)]
#[test]
fn cleanup_keeps_getsid_session_when_process_alive() {
    let (_tmp, ws) = setup();

    // Use our own PID as the session key — guaranteed to be alive.
    let own_pid = std::process::id().to_string();
    let session = Session {
        id: SessionId::new(&own_pid).unwrap(),
        source: SessionSource::Getsid,
    };

    // Reference a conversation that does NOT exist on disk or in memory.
    // With the old logic this would trigger "no live conversations" deletion.
    // With the new logic the alive process takes precedence.
    let ghost_id = ConversationId::try_from(datetime!(2025-07-19 18:00:00 Z)).unwrap();
    ws.activate_session_conversation(&session, ghost_id, datetime!(2025-07-19 18:00:00 Z))
        .unwrap();

    ws.cleanup_stale_files();

    assert!(
        ws.session_active_conversation(&session).is_some(),
        "Getsid session with alive process must survive cleanup even without live conversations"
    );
}

#[cfg(windows)]
#[test]
fn cleanup_removes_stale_hwnd_session() {
    let (_tmp, mut ws) = setup();
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
    ws.cleanup_stale_files();

    assert!(
        ws.session_active_conversation(&session).is_none(),
        "Stale hwnd session should be cleaned up when window handle is dead"
    );
}

#[test]
fn all_active_conversation_ids_across_sessions() {
    let (_tmp, mut ws) = setup();
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
    let (_tmp, ws) = setup();
    assert!(ws.all_active_conversation_ids().is_empty());
}

#[test]
fn cleanup_keeps_session_referencing_conversation_created_after_index_load() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");
    let fixture_storage = jp_storage::Storage::new(&storage_path).unwrap();

    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap()
        .with_local_storage_at(&user_root, "test-ws", "abc")
        .unwrap();
    ws.disable_persistence();

    // Session B references a conversation that exists on disk but was NOT in
    // the workspace's in-memory index (simulates a conversation created by
    // another process after our load_conversation_index call).
    let session_b = Session {
        id: SessionId::new("sess-other-tab").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let conv_other = ConversationId::try_from(datetime!(2025-07-19 16:00:00 Z)).unwrap();

    // Write the conversation to disk directly via a separate Storage handle,
    // bypassing the in-memory state. This is what another `jp` process would do.
    fixture_storage.write_test_conversation(&conv_other, &jp_conversation::Conversation::default());

    // Write a session mapping pointing at that conversation.
    ws.activate_session_conversation(&session_b, conv_other, datetime!(2025-07-19 16:00:00 Z))
        .unwrap();

    // Verify precondition: the conversation is NOT in the in-memory index.
    assert!(
        !ws.conversations().any(|(id, _)| *id == conv_other),
        "Conversation should not be in the in-memory index"
    );

    // Cleanup must NOT delete session_b — the conversation exists on disk.
    ws.cleanup_stale_files();

    assert!(
        ws.session_active_conversation(&session_b).is_some(),
        "Session referencing a conversation created by another process should survive cleanup"
    );
}

#[test]
fn cleanup_keeps_env_session_with_live_conversations() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");
    let fixture_storage = jp_storage::Storage::new(&storage_path).unwrap();

    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap()
        .with_local_storage_at(&user_root, "test-ws", "abc")
        .unwrap();
    ws.disable_persistence();

    let session = Session {
        id: SessionId::new("my-ci-session").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    // Write conversation to disk so the disk-based cleanup scan finds it.
    fixture_storage.write_test_conversation(&id, &jp_conversation::Conversation::default());

    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();

    ws.cleanup_stale_files();

    assert!(
        ws.session_active_conversation(&session).is_some(),
        "Env session with live conversations should not be cleaned up"
    );
}
