use std::sync::Arc;

use chrono::{DateTime, Utc};
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer};
use jp_workspace::{LockResult, Workspace};
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals,
    bootstrap::ExecutionContext,
    cmd::{conversation_id::PositionalIds, time::CreationRange},
};

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::seconds(secs)
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap()
}

fn empty_rm() -> Rm {
    Rm {
        target: PositionalIds::from_targets(vec![]),
        range: CreationRange::default(),
        confirm: ConfirmFlag::default(),
    }
}

fn workspace_with_conversations(ids: &[ConversationId]) -> Workspace {
    let mut ws = Workspace::in_memory("/tmp/jp-cli-rm-test");
    let config = Arc::new(AppConfig::new_test());
    for id in ids {
        ws.create_conversation_with_id(*id, Conversation::default(), config.clone());
    }
    ws
}

/// A `Ctx` over an in-memory workspace holding `ids`.
fn test_ctx(ids: &[ConversationId]) -> Ctx {
    let workspace = workspace_with_conversations(ids);
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);

    Ctx::new(
        ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    )
}

#[test]
fn load_request_is_none_when_range_set() {
    let cmd = Rm {
        range: CreationRange {
            since: Some(ts(1000).into()),
            before: None,
        },
        ..empty_rm()
    };
    assert!(cmd.conversation_load_request().targets.is_none());

    let cmd = Rm {
        range: CreationRange {
            since: None,
            before: Some(ts(1000).into()),
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

// `rm` is destructive; the pre-extraction code path could collect every
// conversation while still satisfying the load-request routing tests above.
// These tests pin the filter integration so a regression that drops the
// `.filter(...)` (or returns the unfiltered iterator) is caught here
// instead of in production.

#[test]
fn resolve_filtered_half_open_range_returns_only_in_range_ids() {
    let before = make_id(500);
    let in_range = make_id(1500);
    let at_until = make_id(2000);
    let after = make_id(2500);
    let ws = workspace_with_conversations(&[before, in_range, at_until, after]);

    let cmd = Rm {
        range: CreationRange {
            since: Some(ts(1000).into()),
            before: Some(ts(2000).into()),
        },
        ..empty_rm()
    };

    let mut ids: Vec<_> = cmd
        .resolve_filtered(&ws)
        .unwrap()
        .iter()
        .map(jp_workspace::ConversationHandle::id)
        .collect();
    ids.sort();

    assert_eq!(ids, vec![in_range]);
}

#[test]
fn resolve_filtered_from_only_includes_from_inclusive() {
    let before = make_id(500);
    let at_from = make_id(1000);
    let after = make_id(2000);
    let ws = workspace_with_conversations(&[before, at_from, after]);

    let cmd = Rm {
        range: CreationRange {
            since: Some(ts(1000).into()),
            before: None,
        },
        ..empty_rm()
    };

    let mut ids: Vec<_> = cmd
        .resolve_filtered(&ws)
        .unwrap()
        .iter()
        .map(jp_workspace::ConversationHandle::id)
        .collect();
    ids.sort();

    assert_eq!(ids, vec![at_from, after]);
}

#[test]
fn resolve_filtered_until_only_excludes_until_exclusive() {
    let before = make_id(500);
    let at_until = make_id(2000);
    let after = make_id(2500);
    let ws = workspace_with_conversations(&[before, at_until, after]);

    let cmd = Rm {
        range: CreationRange {
            since: None,
            before: Some(ts(2000).into()),
        },
        ..empty_rm()
    };

    let mut ids: Vec<_> = cmd
        .resolve_filtered(&ws)
        .unwrap()
        .iter()
        .map(jp_workspace::ConversationHandle::id)
        .collect();
    ids.sort();

    assert_eq!(ids, vec![before]);
}

/// The removal prompt reads the controlling terminal, not stdin, so a run with
/// nobody watching would stop there and wait for a keystroke that never comes.
/// Removal is also the one thing that cannot be undone, so there is no outcome
/// to assume on the user's behalf either.
#[test]
fn removing_without_a_user_fails_unless_confirmation_is_waived() {
    let id = make_id(1000);
    let mut ctx = test_ctx(&[id]);
    ctx.term.interactive = false;

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let LockResult::Acquired(lock) = ctx.workspace.lock_conversation(handle, None).unwrap() else {
        panic!("nothing else holds this conversation");
    };

    let error = confirm_and_remove(&mut ctx, id, &lock, None, false)
        .expect_err("a removal nobody can confirm must not proceed");
    assert_eq!(
        error.message.as_deref(),
        Some(
            "removing conversation jp-c10000 needs a confirmation and nobody is available to give \
             one; pass --no-confirm to remove it without asking"
        )
    );

    // `--no-confirm` is the caller having answered in advance.
    confirm_and_remove(&mut ctx, id, &lock, None, true).expect("nothing left to ask");
}

#[test]
fn resolve_filtered_empty_when_nothing_matches() {
    let ws = workspace_with_conversations(&[make_id(500), make_id(600)]);

    let cmd = Rm {
        range: CreationRange {
            since: Some(ts(1000).into()),
            before: Some(ts(2000).into()),
        },
        ..empty_rm()
    };

    assert!(cmd.resolve_filtered(&ws).unwrap().is_empty());
}
