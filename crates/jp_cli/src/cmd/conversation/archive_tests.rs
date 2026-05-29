use std::sync::Arc;

use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::{AppConfig, conversation::DefaultConversationId};
use jp_conversation::{Conversation, ConversationId};
use jp_workspace::{
    Workspace,
    session::{Session, SessionId, SessionSource},
};

use super::*;
use crate::cmd::{conversation_id::PositionalIds, target::resolve_request, time::CreationRange};

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
        range: CreationRange::default(),
        inactive_since: None,
        yes: false,
    }
}

/// Bug fix: `jp c archive` without targets should resolve to the session's
/// active conversation instead of opening the picker.
/// Mirrors the behavior of `jp c show`.
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
        range: CreationRange {
            from: Some("1d".parse().unwrap()),
            until: None,
        },
        ..empty_archive()
    };

    let req = cmd.conversation_load_request();
    assert!(req.targets.is_none());
}

#[test]
fn until_returns_no_load_request() {
    let cmd = Archive {
        range: CreationRange {
            from: None,
            until: Some("1d".parse().unwrap()),
        },
        ..empty_archive()
    };

    let req = cmd.conversation_load_request();
    assert!(req.targets.is_none());
}

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::seconds(secs)
}

/// Pure range semantics (from-inclusive, until-exclusive) are covered by
/// `CreationRange` tests in `time_tests.rs`.
/// The tests below cover archive-specific composition with `--inactive-since`.
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
        range: CreationRange {
            from: Some(ts(1000).into()),
            until: Some(ts(2000).into()),
        },
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

// `resolve_filtered` is the integration between `matches` and the workspace
// iteration. `matches` is well-covered above, but a regression that broke
// the iteration (e.g. dropping the `.filter`) would still pass the
// `matches_*` tests. Build a workspace with a known set of conversations
// and assert the filter composition selects the expected subset.
fn make_conversation(last_activated_secs: i64) -> Conversation {
    Conversation::default().with_last_activated_at(ts(last_activated_secs))
}

fn workspace_with(conversations: &[(ConversationId, Conversation)]) -> Workspace {
    let mut ws = Workspace::new("/tmp/jp-cli-archive-resolve-test");
    let config = Arc::new(AppConfig::new_test());
    for (id, conv) in conversations {
        ws.create_conversation_with_id(*id, conv.clone(), config.clone());
    }
    ws
}

#[test]
fn resolve_filtered_composes_range_and_inactive_since() {
    // Created in range and inactive long enough: matches.
    let in_range_inactive = (make_id(1500), make_conversation(4000));
    // Created in range but still active: filtered out by inactive_since.
    let in_range_active = (make_id(1700), make_conversation(6000));
    // Inactive but created before the from bound: filtered out.
    let before_from = (make_id(500), make_conversation(4000));
    // Inactive but created at the until bound (exclusive): filtered out.
    let at_until = (make_id(2000), make_conversation(4000));

    let ws = workspace_with(&[
        in_range_inactive.clone(),
        in_range_active,
        before_from,
        at_until,
    ]);

    let cmd = Archive {
        range: CreationRange {
            from: Some(ts(1000).into()),
            until: Some(ts(2000).into()),
        },
        inactive_since: Some(ts(5000).into()),
        ..empty_archive()
    };

    let ids: Vec<_> = cmd
        .resolve_filtered(&ws)
        .unwrap()
        .iter()
        .map(jp_workspace::ConversationHandle::id)
        .collect();

    assert_eq!(ids, vec![in_range_inactive.0]);
}

#[test]
fn resolve_filtered_no_filters_returns_every_conversation() {
    let a = (make_id(100), Conversation::default());
    let b = (make_id(200), Conversation::default());
    let ws = workspace_with(&[a.clone(), b.clone()]);

    let mut ids: Vec<_> = empty_archive()
        .resolve_filtered(&ws)
        .unwrap()
        .iter()
        .map(jp_workspace::ConversationHandle::id)
        .collect();
    ids.sort();

    assert_eq!(ids, vec![a.0, b.0]);
}
