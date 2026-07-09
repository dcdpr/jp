use std::collections::HashSet;

use camino_tempfile::tempdir;
use chrono::TimeZone as _;

use super::*;
use crate::session::{Session, SessionId, SessionSource};

fn env_session(value: &str) -> Session {
    Session {
        id: SessionId::new(value).expect("non-empty"),
        source: SessionSource::env("JP_SESSION"),
    }
}

fn id(s: &str) -> Id {
    Id::from_str(s).expect("valid workspace ID")
}

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0)
        .single()
        .expect("valid timestamp")
}

struct TestStore {
    store: WorkspaceSessionStore,
    // Held for its Drop.
    _tmp: camino_tempfile::Utf8TempDir,
}

fn store() -> TestStore {
    let tmp = tempdir().expect("tempdir");
    let dir = tmp.path().join(SESSIONS_DIR);
    TestStore {
        store: WorkspaceSessionStore::new(dir),
        _tmp: tmp,
    }
}

#[test]
fn record_and_load_round_trip() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(
            &session,
            &id("abc12"),
            Utf8Path::new("/tmp/checkout"),
            at(1_000),
        )
        .unwrap();

    let mapping = t.store.load(&session).expect("mapping stored");
    assert_eq!(mapping.history.len(), 1);
    assert_eq!(mapping.history[0].workspace_id, "abc12");
    assert_eq!(mapping.history[0].root, Utf8Path::new("/tmp/checkout"));
    assert_eq!(mapping.history[0].selected_at, at(1_000));
    assert!(!mapping.sticky);
    assert_eq!(mapping.id, session.id);
    assert_eq!(mapping.source, session.source);

    let active = t.store.active(&session).expect("active selection");
    assert_eq!(active.workspace_id, "abc12");
}

#[test]
fn set_sticky_persists_and_reselection_preserves_it() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(&session, &id("abc12"), Utf8Path::new("/a"), at(1_000))
        .unwrap();
    t.store.set_sticky(&session, true).unwrap();

    assert!(t.store.load(&session).unwrap().sticky);

    // Selecting another workspace keeps the session-level pin.
    t.store
        .record_selection(&session, &id("def34"), Utf8Path::new("/b"), at(2_000))
        .unwrap();
    assert!(t.store.load(&session).unwrap().sticky);

    t.store.set_sticky(&session, false).unwrap();
    assert!(!t.store.load(&session).unwrap().sticky);
}

#[test]
fn set_sticky_without_a_record_is_a_no_op() {
    let t = store();
    let session = env_session("tab-1");

    // Nothing to pin: no record is created.
    t.store.set_sticky(&session, true).unwrap();

    assert!(t.store.load(&session).is_none());
}

#[test]
fn history_is_most_recent_first_and_previous_is_second() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(&session, &id("abc12"), Utf8Path::new("/a"), at(1_000))
        .unwrap();
    t.store
        .record_selection(&session, &id("def34"), Utf8Path::new("/b"), at(2_000))
        .unwrap();

    assert_eq!(t.store.active(&session).unwrap().workspace_id, "def34");
    assert_eq!(t.store.previous(&session).unwrap().workspace_id, "abc12");
}

#[test]
fn reselecting_a_pair_moves_it_to_the_front_without_duplicating() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(&session, &id("abc12"), Utf8Path::new("/a"), at(1_000))
        .unwrap();
    t.store
        .record_selection(&session, &id("def34"), Utf8Path::new("/b"), at(2_000))
        .unwrap();
    t.store
        .record_selection(&session, &id("abc12"), Utf8Path::new("/a"), at(3_000))
        .unwrap();

    let mapping = t.store.load(&session).unwrap();
    assert_eq!(mapping.history.len(), 2);
    assert_eq!(mapping.history[0].workspace_id, "abc12");
    assert_eq!(mapping.history[0].selected_at, at(3_000));
    assert_eq!(mapping.history[1].workspace_id, "def34");
}

#[test]
fn distinct_checkouts_of_one_workspace_are_distinct_entries() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(&session, &id("abc12"), Utf8Path::new("/feature"), at(1_000))
        .unwrap();
    t.store
        .record_selection(&session, &id("abc12"), Utf8Path::new("/main"), at(2_000))
        .unwrap();

    let mapping = t.store.load(&session).unwrap();
    assert_eq!(mapping.history.len(), 2);
    assert_eq!(mapping.history[0].root, Utf8Path::new("/main"));
    assert_eq!(mapping.history[1].root, Utf8Path::new("/feature"));
}

#[test]
fn sticky_flag_survives_reselection() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(&session, &id("abc12"), Utf8Path::new("/a"), at(1_000))
        .unwrap();

    // Flip sticky by hand (the ladder's `A` choice lands in phase 4).
    let mut mapping = t.store.load(&session).unwrap();
    mapping.sticky = true;
    write_json(&t.store.path(&session), &mapping).unwrap();

    t.store
        .record_selection(&session, &id("def34"), Utf8Path::new("/b"), at(2_000))
        .unwrap();

    assert!(t.store.load(&session).unwrap().sticky);
}

#[test]
fn clear_removes_the_record_and_is_idempotent() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(&session, &id("abc12"), Utf8Path::new("/a"), at(1_000))
        .unwrap();
    t.store.clear(&session).unwrap();

    assert!(t.store.load(&session).is_none());

    // Clearing an absent record is not an error.
    t.store.clear(&session).unwrap();
}

#[test]
fn foreign_record_at_the_session_key_is_not_honored() {
    let t = store();
    let session = env_session("tab-1");

    // A record whose stored identity differs from the reading session (hash
    // collision or tampering) is ignored.
    let foreign = WorkspaceSessionMapping {
        history: vec![WorkspaceSelection {
            workspace_id: "abc12".into(),
            root: "/a".into(),
            selected_at: at(1_000),
        }],
        sticky: false,
        id: SessionId::new("other-tab").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    write_json(&t.store.path(&session), &foreign).unwrap();

    assert!(t.store.load(&session).is_none());
}

#[test]
fn sessions_with_same_value_but_different_sources_do_not_collide() {
    let t = store();
    let jp = env_session("42");
    let tmux = Session {
        id: SessionId::new("42").unwrap(),
        source: SessionSource::env("TMUX_PANE"),
    };

    t.store
        .record_selection(&jp, &id("abc12"), Utf8Path::new("/a"), at(1_000))
        .unwrap();
    t.store
        .record_selection(&tmux, &id("def34"), Utf8Path::new("/b"), at(2_000))
        .unwrap();

    assert_eq!(t.store.active(&jp).unwrap().workspace_id, "abc12");
    assert_eq!(t.store.active(&tmux).unwrap().workspace_id, "def34");
}

#[test]
fn cleanup_prunes_env_entries_whose_workspace_has_no_live_root() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(&session, &id("dead1"), Utf8Path::new("/dead"), at(1_000))
        .unwrap();
    t.store
        .record_selection(&session, &id("live1"), Utf8Path::new("/live"), at(2_000))
        .unwrap();

    let live: HashSet<Id> = [id("live1")].into();
    t.store.cleanup(&|id| live.contains(id));

    let mapping = t.store.load(&session).expect("record survives");
    assert_eq!(mapping.history.len(), 1);
    assert_eq!(mapping.history[0].workspace_id, "live1");
}

#[test]
fn cleanup_keeps_entries_of_a_live_workspace_even_when_its_recorded_root_died() {
    let t = store();
    let session = env_session("tab-1");

    // The workspace ID still has *some* live checkout; whether this exact
    // recorded root is alive is not this pass's concern (missing-root
    // recovery re-prompts among the surviving checkouts).
    t.store
        .record_selection(
            &session,
            &id("live1"),
            Utf8Path::new("/removed-worktree"),
            at(1_000),
        )
        .unwrap();

    t.store.cleanup(&|_| true);

    assert_eq!(t.store.load(&session).unwrap().history.len(), 1);
}

#[test]
fn cleanup_removes_env_record_with_no_live_workspace_at_all() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(&session, &id("dead1"), Utf8Path::new("/a"), at(1_000))
        .unwrap();
    t.store
        .record_selection(&session, &id("dead2"), Utf8Path::new("/b"), at(2_000))
        .unwrap();

    t.store.cleanup(&|_| false);

    assert!(t.store.load(&session).is_none());
}

#[test]
fn cleanup_keeps_record_of_a_live_getsid_process_unconditionally() {
    let t = store();
    // Our own process is alive by definition.
    #[expect(clippy::cast_possible_wrap, reason = "test PIDs fit in i32")]
    let session = Session::getsid(std::process::id() as i32);

    t.store
        .record_selection(&session, &id("dead1"), Utf8Path::new("/gone"), at(1_000))
        .unwrap();

    // Even with no live workspace anywhere, a live process keeps its record.
    t.store.cleanup(&|_| false);

    assert_eq!(t.store.load(&session).unwrap().history.len(), 1);
}

#[test]
fn cleanup_prunes_unreadable_records() {
    let t = store();
    let session = env_session("tab-1");

    // Ensure the directory exists, then plant garbage.
    t.store
        .record_selection(&session, &id("live1"), Utf8Path::new("/a"), at(1_000))
        .unwrap();
    let garbage = t.store.dir.join("garbage.json");
    fs::write(&garbage, b"not json").unwrap();

    t.store.cleanup(&|_| true);

    assert!(!garbage.exists());
    assert!(t.store.load(&session).is_some(), "healthy record untouched");
}

#[test]
fn cleanup_on_missing_directory_is_a_no_op() {
    let t = store();
    // No record was ever written; the sessions/ directory does not exist.
    t.store.cleanup(&|_| true);
}

#[test]
fn on_disk_shape_matches_rfd_087() {
    let t = store();
    let session = env_session("tab-1");

    t.store
        .record_selection(
            &session,
            &id("abc12"),
            Utf8Path::new("/checkout"),
            at(1_000),
        )
        .unwrap();

    let raw: serde_json::Value = read_json(&t.store.path(&session)).unwrap();
    let entry = &raw["history"][0];
    assert_eq!(entry["workspace_id"], "abc12");
    assert_eq!(entry["root"], "/checkout");
    assert!(entry["selected_at"].is_string());
    assert_eq!(raw["sticky"], false);
    assert_eq!(raw["source"]["type"], "env");
}
