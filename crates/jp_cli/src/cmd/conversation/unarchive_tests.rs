use std::{
    fs,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use camino_tempfile::Utf8TempDir;
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::{FsStorageBackend, LockBackend as _, RecoveryBackend as _, Repair};
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{Globals, bootstrap::ExecutionContext, cmd::target::ConversationTarget};

fn id() -> ConversationId {
    ConversationId::try_from_dirname("17941219991").unwrap()
}

/// A `Ctx` over a workspace on disk holding one archived conversation.
fn archived(interactive: bool) -> (Utf8TempDir, Ctx, Arc<FsStorageBackend>) {
    let tmp = camino_tempfile::tempdir().unwrap();
    let fs = Arc::new(FsStorageBackend::new(&tmp.path().join(".jp")).unwrap());
    fs.write_test_conversation(&id(), &Conversation::default());

    let mut ws = Workspace::in_memory(tmp.path()).with_backend(Arc::clone(&fs));
    ws.load_conversation_index();
    let handle = ws.acquire_conversation(&id()).unwrap();
    ws.archive_conversation(ws.test_lock(handle).into_mut())
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut ctx = Ctx::new(
        ExecutionContext::for_workspace(&ws),
        ws,
        Some(Arc::clone(&fs)),
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );
    ctx.term.interactive = interactive;

    (tmp, ctx, fs)
}

// `jp doctor` suggests unarchiving an archived conversation that fails to load:
// back in the live store, the fixes for live conversations reach it.
#[test]
fn an_archived_conversation_that_fails_to_load_is_unarchived() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let fs = Arc::new(FsStorageBackend::new(&tmp.path().join(".jp")).unwrap());
    let conversations = tmp.path().join(".jp").join("conversations");
    let archived = conversations.join(".archive").join("17941219991");
    fs::create_dir_all(&archived).unwrap();
    fs::write(
        archived.join("events.json"),
        r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request","content":"a"}]"#,
    )
    .unwrap();

    let mut ws = Workspace::in_memory(tmp.path()).with_backend(Arc::clone(&fs));
    ws.load_conversation_index();
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut ctx = Ctx::new(
        ExecutionContext::for_workspace(&ws),
        ws,
        Some(Arc::clone(&fs)),
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );

    unarchive(&mut ctx).unwrap();

    let diagnosis = fs.diagnose().unwrap();
    let found: Vec<_> = diagnosis
        .invalid
        .iter()
        .map(|diagnosed| (diagnosed.conversation.path(), diagnosed.repair))
        .collect();
    assert!(!archived.exists());
    assert_eq!(found, vec![(
        conversations.join("17941219991"),
        Some(Repair::RebuildMetadata)
    )]);
}

fn unarchive(ctx: &mut Ctx) -> Output {
    let cmd = Unarchive {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id())]),
    };
    Runtime::new().unwrap().block_on(cmd.run(ctx))
}

// With someone there to wait, the run waits for the holder to let go.
#[test]
fn waits_for_a_conversation_another_process_holds() {
    // On a thread of its own, with a deadline: a wait that never sees the lock
    // free ends at the contention prompt, which blocks on a terminal the test
    // does not have.
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let (_tmp, mut ctx, fs) = archived(true);
        let lock = fs.try_lock(&id().to_string(), None).unwrap().unwrap();
        let holder = thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            drop(lock);
        });

        let result = unarchive(&mut ctx);
        holder.join().unwrap();
        let live = ctx.workspace.acquire_conversation(&id()).is_ok();
        done.send((result.map_err(|error| error.message), live))
            .unwrap();
    });

    let (result, live) = finished
        .recv_timeout(Duration::from_secs(5))
        .expect("the unarchive did not finish once the holder let go");
    assert_eq!(result, Ok(()));
    assert!(live);
}

#[test]
fn fails_on_a_held_conversation_when_nobody_can_wait() {
    let (_tmp, mut ctx, fs) = archived(false);
    let _lock = fs.try_lock(&id().to_string(), None).unwrap().unwrap();

    let error = unarchive(&mut ctx).unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some("Timed out waiting for lock on conversation jp-c17941219991")
    );
    assert!(ctx.workspace.acquire_conversation(&id()).is_err());
}
