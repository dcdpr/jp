use std::sync::Arc;

use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer};
use jp_workspace::{
    LockResult, Workspace,
    session::{Session, SessionId, SessionSource},
};
use tokio::runtime::Runtime;

use super::*;
use crate::{Globals, cmd::conversation_id::PositionalIds, ctx::Ctx};

fn original_last_activated() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap()
}

fn test_session() -> Session {
    Session {
        id: SessionId::new("jp-cli-use-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    }
}

/// Build a `Ctx` with an in-memory workspace, a session, and a single
/// conversation whose `last_activated_at` is pinned to `ORIGINAL_LAST_ACTIVATED`.
fn setup(id: ConversationId) -> Ctx {
    let mut workspace = Workspace::new("/tmp/jp-cli-use-test");
    workspace.create_conversation_with_id(
        id,
        Conversation {
            last_activated_at: original_last_activated(),
            ..Default::default()
        },
        Arc::new(AppConfig::new_test()),
    );

    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    let mut ctx = Ctx::new(
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        Some(test_session()),
        printer,
    );
    // Pin "now" to a specific point so we can assert on the bump.
    ctx.set_now(Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap());
    ctx
}

/// Without contention, `Use::run` bumps the conversation's `last_activated_at`
/// AND records the session mapping. This is the bug fix in its happy path.
#[test]
fn run_without_contention_bumps_last_activated_at() {
    let id = make_id(1000);
    let mut ctx = setup(id);
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();

    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
    };
    cmd.run(&mut ctx, vec![handle]).unwrap();

    // Session mapping points at the conversation.
    let session = ctx.session.clone().unwrap();
    assert_eq!(
        ctx.workspace.session_active_conversation(&session),
        Some(id)
    );

    // Conversation's `last_activated_at` was bumped to ctx.now().
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let meta = ctx.workspace.metadata(&h).unwrap();
    assert_eq!(meta.last_activated_at, ctx.now());
}

/// With contention (the conversation is already locked by something else in
/// this process), `Use::run` falls back to writing only the session mapping.
/// `last_activated_at` is left at the lock holder's value.
#[test]
fn run_with_contention_skips_metadata_bump() {
    let id = make_id(2000);
    let mut ctx = setup(id);

    // Hold an exclusive lock on the conversation. The InMemory lock backend
    // tracks held locks per conversation ID, so a second `lock_conversation`
    // call on the same ID will return `AlreadyLocked` — exactly the path
    // the fallback handles.
    let blocking_handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let _held = match ctx
        .workspace
        .lock_conversation(blocking_handle, ctx.session.as_ref())
        .unwrap()
    {
        LockResult::Acquired(lock) => lock,
        LockResult::AlreadyLocked(_) => panic!("first lock attempt must succeed"),
    };

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
    };
    cmd.run(&mut ctx, vec![handle]).unwrap();

    // Session mapping was still written — that's the user-visible effect of
    // `jp c use X`, and it must succeed even when the conversation is busy.
    let session = ctx.session.clone().unwrap();
    assert_eq!(
        ctx.workspace.session_active_conversation(&session),
        Some(id)
    );

    // `last_activated_at` was NOT touched by the contended path. The lock
    // holder is responsible for writing this field; we don't fight them
    // for it.
    let h = ctx.workspace.acquire_conversation(&id).unwrap();
    let meta = ctx.workspace.metadata(&h).unwrap();
    assert_eq!(
        meta.last_activated_at,
        original_last_activated(),
        "contended path must leave last_activated_at untouched"
    );
}
