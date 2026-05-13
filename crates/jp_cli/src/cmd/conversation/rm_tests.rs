use chrono::{DateTime, Utc};

use super::*;
use crate::cmd::{conversation_id::PositionalIds, time::CreationRange};

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::seconds(secs)
}

fn empty_rm() -> Rm {
    Rm {
        target: PositionalIds::from_targets(vec![]),
        range: CreationRange::default(),
        yes: false,
    }
}

#[test]
fn load_request_is_none_when_range_set() {
    let cmd = Rm {
        range: CreationRange {
            from: Some(ts(1000).into()),
            until: None,
        },
        ..empty_rm()
    };
    assert!(cmd.conversation_load_request().targets.is_none());

    let cmd = Rm {
        range: CreationRange {
            from: None,
            until: Some(ts(1000).into()),
        },
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
