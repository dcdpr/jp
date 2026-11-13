use std::{fs, sync::Arc};

use camino_tempfile::tempdir;
use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationEvent, ConversationId, event::ChatRequest};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::{FsStorageBackend, InMemoryStorageBackend, LoadBackend as _};
use jp_workspace::{
    LockResult, Workspace,
    session::{Session, SessionId, SessionSource},
};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals,
    cmd::{
        conversation_id::PositionalIds,
        target::ConversationTarget,
        time::{CreationRange, TimeThreshold},
    },
    ctx::Ctx,
};

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
/// conversation whose `last_activated_at` is pinned to
/// `ORIGINAL_LAST_ACTIVATED`.
fn setup(id: ConversationId) -> Ctx {
    let mut workspace = Workspace::in_memory("/tmp/jp-cli-use-test");
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
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
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
/// AND records the session mapping.
/// This is the bug fix in its happy path.
#[test]
fn run_without_contention_bumps_last_activated_at() {
    let id = make_id(1000);
    let mut ctx = setup(id);
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();

    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
        grep: None,
        range: CreationRange::default(),
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
        grep: None,
        range: CreationRange::default(),
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

// --- Filter mode (`--grep`, `--created-since`, `--created-before`) ----------
//
// Filter mode resolves handles internally instead of going through the
// standard pipeline. These tests exercise the N=1 short-circuit path that
// can be driven without the interactive picker.

fn setup_multi(entries: Vec<(ConversationId, Conversation, Vec<ConversationEvent>)>) -> Ctx {
    let mut workspace = Workspace::in_memory("/tmp/jp-cli-use-filter-test");
    let config = Arc::new(AppConfig::new_test());

    for (id, conversation, _) in &entries {
        workspace.create_conversation_with_id(*id, conversation.clone(), config.clone());
    }

    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        Some(test_session()),
        printer,
    );
    ctx.set_now(Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap());

    for (id, _, evts) in entries {
        let h = ctx.workspace.acquire_conversation(&id).unwrap();
        let lock = ctx.workspace.test_lock(h);
        lock.as_mut().update_events(|e| e.extend(evts));
    }

    ctx
}

/// `--grep` narrowing to a single matching conversation activates it without
/// prompting.
#[test]
fn filter_grep_single_match_activates_directly() {
    let id_match = make_id(1000);
    let id_other = make_id(2000);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

    let mut ctx = setup_multi(vec![
        (id_match, Conversation::default(), vec![
            ConversationEvent::new(ChatRequest::from("the deployment failed today"), ts),
        ]),
        (id_other, Conversation::default(), vec![
            ConversationEvent::new(ChatRequest::from("unrelated"), ts),
        ]),
    ]);

    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
        grep: Some("deployment".into()),
        range: CreationRange::default(),
    };
    cmd.run(&mut ctx, vec![]).unwrap();

    let session = ctx.session.clone().unwrap();
    assert_eq!(
        ctx.workspace.session_active_conversation(&session),
        Some(id_match)
    );
}

/// `--grep` with a literal ID + matching pattern activates the ID.
/// Composing the two is benign — grep filters the single-element candidate set
/// down to itself.
#[test]
fn filter_grep_with_literal_id_match_activates() {
    let id = make_id(3000);
    let mut ctx = setup_multi(vec![(id, Conversation::default(), vec![
        ConversationEvent::new(
            ChatRequest::from("rollout strategy"),
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        ),
    ])]);

    let cmd = Use {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        grep: Some("rollout".into()),
        range: CreationRange::default(),
    };
    cmd.run(&mut ctx, vec![]).unwrap();

    let session = ctx.session.clone().unwrap();
    assert_eq!(
        ctx.workspace.session_active_conversation(&session),
        Some(id)
    );
}

/// `--grep` with a literal ID + non-matching pattern errors with the standard
/// "no conversations match" error — no silent activation, no special-case.
#[test]
fn filter_grep_with_literal_id_no_match_errors() {
    let id = make_id(4000);
    let mut ctx = setup_multi(vec![(id, Conversation::default(), vec![
        ConversationEvent::new(
            ChatRequest::from("hello world"),
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        ),
    ])]);

    let cmd = Use {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id)]),
        grep: Some("nonexistent".into()),
        range: CreationRange::default(),
    };
    let err = cmd.run(&mut ctx, vec![]).unwrap_err();
    assert!(
        format!("{err:?}").contains("no conversations match"),
        "expected NotFound error, got: {err:?}"
    );
}

/// `--from` clips the candidate set by creation timestamp before grep runs.
/// Combined with `--grep` matching one survivor, this activates directly.
#[test]
fn filter_range_narrows_then_grep_matches() {
    let id_old = make_id(1_000);
    let id_new = make_id(2_000_000_000);
    let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

    let mut ctx = setup_multi(vec![
        (id_old, Conversation::default(), vec![
            ConversationEvent::new(ChatRequest::from("shared-marker old"), ts),
        ]),
        (id_new, Conversation::default(), vec![
            ConversationEvent::new(ChatRequest::from("shared-marker new"), ts),
        ]),
    ]);

    // `--from` at a timestamp between the two IDs keeps only id_new.
    let cutoff = TimeThreshold::from(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
        grep: Some("shared-marker".into()),
        range: CreationRange {
            since: Some(cutoff),
            before: None,
        },
    };
    cmd.run(&mut ctx, vec![]).unwrap();

    let session = ctx.session.clone().unwrap();
    assert_eq!(
        ctx.workspace.session_active_conversation(&session),
        Some(id_new)
    );
}

#[test]
fn filter_grep_drops_a_match_whose_stream_does_not_load() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let backend = FsStorageBackend::new(&root.join("storage")).unwrap();

    let id_ok = make_id(1000);
    let id_broken = make_id(2000);
    let config = Arc::new(AppConfig::new_test());

    {
        let mut ws = Workspace::in_memory(&root).with_backend(Arc::new(backend.clone()));
        for id in [id_ok, id_broken] {
            ws.create_conversation_with_id(id, Conversation::default(), config.clone());
            let h = ws.acquire_conversation(&id).unwrap();
            let mut conv = ws.test_lock(h).into_mut();
            conv.update_events(|events| events.start_turn(ChatRequest::from("the deployment")));
            conv.flush().unwrap();
        }
    }

    // A config delta whose `op` this build does not know. The event search
    // skips config deltas, and the stream load fails on this one.
    let events_path = backend.conversation_events_path(&id_broken).unwrap();
    let mut events: Vec<Value> = serde_json::from_slice(&fs::read(&events_path).unwrap()).unwrap();
    events.push(json!({
        "type": "config_delta",
        "timestamp": "2025-01-01 00:00:00.0",
        "op": "from_the_future"
    }));
    fs::write(&events_path, serde_json::to_string(&events).unwrap()).unwrap();

    // Startup validation checks structure only, so it does not trash the
    // conversation before `c use` runs.
    assert!(!backend.sanitize().unwrap().has_repairs());

    // `FsStorageBackend` stores session mappings in user storage, which is not
    // configured in this fixture, so keep sessions in memory.
    let mut workspace = Workspace::in_memory(&root)
        .with_backend(Arc::new(backend))
        .with_sessions(Arc::new(InMemoryStorageBackend::new()));
    workspace.load_conversation_index();

    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        Some(test_session()),
        printer,
    );
    ctx.set_now(Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap());

    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
        grep: Some("deployment".into()),
        range: CreationRange::default(),
    };
    cmd.run(&mut ctx, vec![]).unwrap();

    let session = ctx.session.clone().unwrap();
    assert_eq!(
        ctx.workspace.session_active_conversation(&session),
        Some(id_ok)
    );
}

/// Filter mode is opted into by either `--grep` or `--range`.
/// The standard `conversation_load_request` short-circuits to `none()` so
/// handle resolution happens inside `Use`.
#[test]
fn filter_mode_skips_standard_resolution() {
    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
        grep: Some("any".into()),
        range: CreationRange::default(),
    };
    assert!(
        cmd.conversation_load_request().targets.is_none(),
        "filter mode must return ConversationLoadRequest::none()"
    );

    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
        grep: None,
        range: CreationRange {
            since: Some(TimeThreshold::from(
                Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            )),
            before: None,
        },
    };
    assert!(cmd.conversation_load_request().targets.is_none());

    // Bare `c use` (no filter) goes through the standard pipeline.
    let cmd = Use {
        target: PositionalIds::from_targets(vec![]),
        grep: None,
        range: CreationRange::default(),
    };
    assert!(cmd.conversation_load_request().targets.is_some());
}
