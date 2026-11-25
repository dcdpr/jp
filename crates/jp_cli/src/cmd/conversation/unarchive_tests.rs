use std::{
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use camino_tempfile::{Utf8TempDir, tempdir};
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::{FsStorageBackend, LockBackend as _};
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals, bootstrap::ExecutionContext, cli::parse_args, cmd::target::ConversationTarget,
};

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

fn unarchive(ctx: &mut Ctx) -> Output {
    let cmd = Unarchive {
        target: PositionalIds::from_targets(vec![ConversationTarget::Id(id())]),
    };
    Runtime::new().unwrap().block_on(cmd.run(ctx))
}

#[test]
fn waits_for_a_conversation_another_process_holds() {
    // Run on a dedicated thread with a timeout because a failed wait falls
    // through to the interactive contention prompt, which would block when no
    // terminal is attached.
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

/// Regression: the second copy of a repeated ID found the conversation already
/// restored by the first, and reported it as never having been archived.
#[test]
fn a_repeated_id_is_unarchived_once() {
    let tmp = tempdir().unwrap();
    let (printer, out, _) = Printer::memory(OutputFormat::Text);
    let fs = Arc::new(FsStorageBackend::new(&tmp.path().join(".jp")).unwrap());
    let workspace = Workspace::in_memory(tmp.path()).with_backend(fs);
    let mut ctx = Ctx::new(
        ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );

    let id = ConversationId::try_from(ctx.now()).unwrap();
    ctx.workspace
        .create_conversation_with_id(id, Conversation::default(), ctx.config());
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let mut conv = ctx.workspace.test_lock(handle).into_mut();
    conv.update_metadata(|_| {});
    conv.flush().unwrap();
    drop(conv);
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let lock = ctx.workspace.test_lock(handle);
    ctx.workspace.archive_conversation(lock.into_mut()).unwrap();

    let id_arg = id.to_string();
    let cmd = parse_args::<Unarchive>(&[&id_arg, &id_arg]).unwrap();
    Runtime::new().unwrap().block_on(cmd.run(&mut ctx)).unwrap();
    ctx.printer.flush();

    assert_eq!(*out.lock(), format!("Conversation {id} unarchived.\n"));
}
