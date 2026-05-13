use std::sync::Arc;

use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::{AppConfig, conversation::DefaultConversationId};
use jp_conversation::{Conversation, ConversationId};
use jp_workspace::{
    Workspace,
    session::{Session, SessionId, SessionSource},
};

use super::*;
use crate::cmd::{conversation_id::PositionalIds, target::resolve_request};

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap()
}

fn test_session() -> Session {
    Session {
        id: SessionId::new("jp-cli-archive-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    }
}

/// Build a workspace with a conversation that the session has activated.
fn workspace_with_active_conversation(id: ConversationId) -> (Workspace, Session) {
    let mut ws = Workspace::new("/tmp/jp-cli-archive-test");
    ws.create_conversation_with_id(id, Conversation::default(), Arc::new(AppConfig::new_test()));

    let session = test_session();
    ws.record_session_activation(
        &session,
        id,
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    )
    .unwrap();

    (ws, session)
}

/// Default constructor for tests — no targets, no filters, no `--yes`.
fn empty_archive() -> Archive {
    Archive {
        target: PositionalIds::from_targets(vec![]),
        from: None,
        until: None,
        inactive_since: None,
        yes: false,
    }
}

/// Bug fix: `jp c archive` without targets should resolve to the session's
/// active conversation instead of opening the picker. Mirrors the behavior
/// of `jp c show`.
#[test]
fn no_target_resolves_to_session_active_conversation() {
    let id = make_id(1000);
    let (ws, session) = workspace_with_active_conversation(id);

    let cmd = empty_archive();

    let handles = resolve_request(
        &cmd.conversation_load_request(),
        &ws,
        Some(&session),
        DefaultConversationId::Ask,
    )
    .unwrap();

    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].id(), id);
}

/// Explicit ID still routes through the explicit target path.
#[test]
fn explicit_target_resolves_to_that_conversation() {
    use crate::cmd::target::ConversationTarget;

    let id = make_id(2000);
    let (ws, session) = workspace_with_active_conversation(id);

    let cmd = Archive {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        ..empty_archive()
    };

    let handles = resolve_request(
        &cmd.conversation_load_request(),
        &ws,
        Some(&session),
        DefaultConversationId::Ask,
    )
    .unwrap();

    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].id(), id);
}

/// Any filter flag skips the load request — the command resolves its own
/// conversations internally.
#[test]
fn inactive_since_returns_no_load_request() {
    let cmd = Archive {
        inactive_since: Some("1d".parse().unwrap()),
        ..empty_archive()
    };

    let req = cmd.conversation_load_request();
    assert!(req.targets.is_none());
}

#[test]
fn from_returns_no_load_request() {
    let cmd = Archive {
        from: Some("1d".parse().unwrap()),
        ..empty_archive()
    };

    let req = cmd.conversation_load_request();
    assert!(req.targets.is_none());
}

#[test]
fn until_returns_no_load_request() {
    let cmd = Archive {
        until: Some("1d".parse().unwrap()),
        ..empty_archive()
    };

    let req = cmd.conversation_load_request();
    assert!(req.targets.is_none());
}

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::seconds(secs)
}

#[test]
fn matches_from_inclusive() {
    let cmd = Archive {
        from: Some(ts(1000).into()),
        ..empty_archive()
    };

    // Strictly before: excluded.
    assert!(!cmd.matches(make_id(999), &Conversation::default()));
    // Equal: included (>=).
    assert!(cmd.matches(make_id(1000), &Conversation::default()));
    // After: included.
    assert!(cmd.matches(make_id(1001), &Conversation::default()));
}

#[test]
fn matches_until_exclusive() {
    let cmd = Archive {
        until: Some(ts(2000).into()),
        ..empty_archive()
    };

    assert!(cmd.matches(make_id(1999), &Conversation::default()));
    // Equal: excluded (<).
    assert!(!cmd.matches(make_id(2000), &Conversation::default()));
    assert!(!cmd.matches(make_id(2001), &Conversation::default()));
}

#[test]
fn matches_inactive_since_uses_last_activated_at() {
    let cmd = Archive {
        inactive_since: Some(ts(5000).into()),
        ..empty_archive()
    };

    let active_recently = Conversation::default().with_last_activated_at(ts(6000));
    let active_long_ago = Conversation::default().with_last_activated_at(ts(4000));

    // Recently active: NOT inactive-since, so excluded.
    assert!(!cmd.matches(make_id(1), &active_recently));
    // Long inactive: included.
    assert!(cmd.matches(make_id(1), &active_long_ago));
}

#[test]
fn matches_filters_and_compose() {
    let cmd = Archive {
        from: Some(ts(1000).into()),
        until: Some(ts(2000).into()),
        inactive_since: Some(ts(5000).into()),
        ..empty_archive()
    };

    let in_range_inactive = Conversation::default().with_last_activated_at(ts(4000));
    let in_range_active = Conversation::default().with_last_activated_at(ts(6000));
    let out_of_range_inactive = Conversation::default().with_last_activated_at(ts(4000));

    // Created at 1500 (in [1000, 2000)), inactive since 5000 -> match.
    assert!(cmd.matches(make_id(1500), &in_range_inactive));
    // Same range but recently active -> no match (fails inactive-since).
    assert!(!cmd.matches(make_id(1500), &in_range_active));
    // Inactive but out of range -> no match (fails until).
    assert!(!cmd.matches(make_id(2500), &out_of_range_inactive));
    // Inactive but before from -> no match (fails from).
    assert!(!cmd.matches(make_id(500), &out_of_range_inactive));
}

#[test]
fn matches_no_filters_accepts_everything() {
    let cmd = empty_archive();
    assert!(cmd.matches(make_id(0), &Conversation::default()));
    assert!(cmd.matches(make_id(1_000_000_000), &Conversation::default()));
}
