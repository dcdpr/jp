use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;

use super::*;
use crate::cmd::conversation_id::PositionalIds;

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap()
}

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::seconds(secs)
}

fn empty_rm() -> Rm {
    Rm {
        target: PositionalIds::from_targets(vec![]),
        from: None,
        until: None,
        yes: false,
    }
}

#[test]
fn matches_from_inclusive() {
    let cmd = Rm {
        from: Some(ts(1000).into()),
        ..empty_rm()
    };

    assert!(!cmd.matches(make_id(999)));
    assert!(cmd.matches(make_id(1000)));
    assert!(cmd.matches(make_id(1001)));
}

#[test]
fn matches_until_exclusive() {
    let cmd = Rm {
        until: Some(ts(2000).into()),
        ..empty_rm()
    };

    assert!(cmd.matches(make_id(1999)));
    assert!(!cmd.matches(make_id(2000)));
    assert!(!cmd.matches(make_id(2001)));
}

#[test]
fn matches_half_open_range() {
    let cmd = Rm {
        from: Some(ts(1000).into()),
        until: Some(ts(2000).into()),
        ..empty_rm()
    };

    assert!(!cmd.matches(make_id(999)));
    assert!(cmd.matches(make_id(1000)));
    assert!(cmd.matches(make_id(1500)));
    assert!(!cmd.matches(make_id(2000)));
    assert!(!cmd.matches(make_id(2500)));
}

#[test]
fn matches_no_filters_accepts_everything() {
    let cmd = empty_rm();
    assert!(cmd.matches(make_id(0)));
    assert!(cmd.matches(make_id(1_000_000_000)));
}

#[test]
fn load_request_is_none_when_range_set() {
    let cmd = Rm {
        from: Some(ts(1000).into()),
        ..empty_rm()
    };
    assert!(cmd.conversation_load_request().targets.is_none());

    let cmd = Rm {
        until: Some(ts(1000).into()),
        ..empty_rm()
    };
    assert!(cmd.conversation_load_request().targets.is_none());
}

#[test]
fn load_request_uses_target_when_no_range() {
    let cmd = empty_rm();
    // No range, no explicit targets — falls back to session resolution.
    assert!(cmd.conversation_load_request().targets.is_some());
}
