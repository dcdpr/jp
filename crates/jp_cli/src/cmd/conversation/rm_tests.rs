use std::{fs, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use chrono::{DateTime, Utc};
use crossterm::style::Stylize as _;
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_storage::{
    backend::{FsStorageBackend, LockBackend as _, NullLockBackend, NullPersistBackend},
    validate::{DirContents, FileError, ManagedFile, ValidationError},
};
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals,
    bootstrap::ExecutionContext,
    cli::parse_args,
    cmd::{conversation_id::PositionalIds, target::ConversationTarget, time::CreationRange},
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

/// `--no-confirm` is the caller having answered in advance, so the removal runs
/// without a user present.
/// The prompt's own refusal to proceed unasked is covered in `shared::confirm`.
#[test]
fn a_waived_confirmation_removes_without_a_user() {
    let id = make_id(1000);
    let mut ctx = test_ctx(&[id]);
    ctx.term.interactive = false;

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let removed = Runtime::new()
        .unwrap()
        .block_on(remove(&mut ctx, handle, None, true))
        .expect("nothing left to ask");

    assert!(removed, "the removal is reported to the caller");
    assert!(
        ctx.workspace.acquire_conversation(&id).is_err(),
        "the conversation is gone"
    );
}

struct Store {
    _tmp: Utf8TempDir,
    ctx: Ctx,
    out: SharedBuffer,
    fs: Arc<FsStorageBackend>,
    workspace: Utf8PathBuf,
    user: Utf8PathBuf,
}

/// A `Ctx` over a workspace on disk with user-local storage, and nobody
/// available to answer a prompt.
///
/// Without `persist`, the workspace writes and locks through the null backends,
/// as `load_workspace` sets it up for `--no-persist`.
fn store(persist: bool) -> Store {
    let tmp = camino_tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let fs = Arc::new(
        FsStorageBackend::new(&root.join(".jp"))
            .unwrap()
            .with_user_storage(&root.join("user"), None, "abc")
            .unwrap(),
    );
    let workspace = root.join(".jp").join("conversations");
    let user = root.join("user").join("abc").join("conversations");

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut ws = Workspace::in_memory(&root).with_backend(Arc::clone(&fs));
    if !persist {
        ws = ws
            .with_persist(Arc::new(NullPersistBackend))
            .with_locker(Arc::new(NullLockBackend));
    }
    let mut ctx = Ctx::new(
        ExecutionContext::for_workspace(&ws),
        ws,
        Some(Arc::clone(&fs)),
        Runtime::new().unwrap(),
        Globals {
            persist,
            ..Globals::default()
        },
        AppConfig::new_test(),
        None,
        printer,
    );
    ctx.term.interactive = false;

    Store {
        _tmp: tmp,
        ctx,
        out,
        fs,
        workspace,
        user,
    }
}

fn write_dir(conversations: &Utf8Path, dirname: &str, files: &[(&str, &str)]) -> Utf8PathBuf {
    let dir = conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        fs::write(dir.join(name), content).unwrap();
    }
    dir
}

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

fn rm_ids(dirnames: &[&str], confirm: &[&str]) -> Rm {
    Rm {
        target: PositionalIds::from_targets(
            dirnames
                .iter()
                .map(|dirname| ConversationTarget::Id(id(dirname)))
                .collect(),
        ),
        range: CreationRange::default(),
        confirm: parse_args::<ConfirmFlag>(confirm).unwrap(),
    }
}

fn run(store: &mut Store, rm: Rm) -> Output {
    let handles = vec![];
    let result = Runtime::new()
        .unwrap()
        .block_on(rm.run(&mut store.ctx, handles));
    store.ctx.printer.flush();
    result
}

// Typed IDs reach conversations the conversation list leaves out: one whose
// files are broken and one that was never stored, in either storage root.
#[test]
fn removes_conversations_that_fail_to_load_by_id() {
    let mut s = store(true);
    let broken = write_dir(&s.workspace, "17941219991-a-title", &[(
        "metadata.json",
        "not json",
    )]);
    let draft = write_dir(&s.user, "17941219992", &[("QUERY_MESSAGE.md", "typed")]);

    run(
        &mut s,
        rm_ids(&["17941219991", "17941219992"], &["--no-confirm"]),
    )
    .unwrap();

    assert_eq!(s.out.lock().as_str(), "Conversation(s) removed.\n");
    assert!(!broken.exists());
    assert!(!draft.exists());
}

// The removal deletes a conversation's own directories and nothing else, however
// close another entry's name is.
#[test]
fn removing_by_id_leaves_everything_else_alone() {
    let mut s = store(true);
    let target = write_dir(&s.workspace, "17941219991", &[(
        "metadata.json",
        "not json",
    )]);
    let keep = [
        write_dir(&s.workspace, "179412199915", &[(
            "metadata.json",
            "not json",
        )]),
        write_dir(&s.workspace.join(".archive"), "17941219991", &[(
            "events.json",
            "[]",
        )]),
        write_dir(&s.workspace.join(".trash"), "17941219991", &[(
            "events.json",
            "[]",
        )]),
        write_dir(&s.workspace, "not-an-id", &[("notes", "")]),
    ];

    run(&mut s, rm_ids(&["17941219991"], &["--no-confirm"])).unwrap();

    assert!(!target.exists());
    for dir in keep {
        assert!(dir.is_dir(), "{dir} must survive");
    }
    assert!(s.workspace.is_dir());
}

// The conversations `rm` always handled still go the usual way.
#[test]
fn removes_a_loading_conversation_by_id() {
    let mut s = store(true);
    s.fs.write_test_conversation(&id("17941219993"), &Conversation::default());
    s.ctx.workspace.load_conversation_index();

    run(&mut s, rm_ids(&["17941219993"], &["--no-confirm"])).unwrap();

    assert!(
        s.ctx
            .workspace
            .acquire_conversation(&id("17941219993"))
            .is_err()
    );
    assert!(!s.workspace.join("17941219993").exists());
}

#[test]
fn an_id_with_no_directory_is_not_found() {
    let mut s = store(true);
    let expected: cmd::Error = crate::error::Error::from(
        s.ctx
            .workspace
            .acquire_conversation(&id("17941219994"))
            .unwrap_err(),
    )
    .into();

    let error = run(&mut s, rm_ids(&["17941219994"], &["--no-confirm"])).unwrap_err();

    assert_eq!(error.message, expected.message);
}

#[test]
fn refuses_a_conversation_another_process_holds() {
    let mut s = store(true);
    let dir = write_dir(&s.workspace, "17941219991", &[(
        "metadata.json",
        "not json",
    )]);
    let _lock =
        s.fs.try_lock(&id("17941219991").to_string(), None)
            .unwrap()
            .unwrap();

    let error = run(&mut s, rm_ids(&["17941219991"], &["--no-confirm"])).unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some("Conversation jp-c17941219991 is in use by another process.")
    );
    assert!(dir.is_dir());
}

// Removal cannot be undone, so it is never assumed when nobody can be asked.
#[test]
fn asks_before_removing_and_fails_when_nobody_can_answer() {
    let mut s = store(true);
    let dir = write_dir(&s.workspace, "17941219991", &[(
        "metadata.json",
        "not json",
    )]);

    let error = run(&mut s, rm_ids(&["17941219991"], &[])).unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some(
            "removing conversation jp-c17941219991 needs a confirmation and nobody is available \
             to give one; pass --no-confirm to remove it without asking"
        )
    );
    assert!(dir.is_dir());
}

#[test]
fn no_persist_removes_nothing_and_says_so() {
    let mut s = store(false);
    let dir = write_dir(&s.workspace, "17941219991", &[(
        "metadata.json",
        "not json",
    )]);

    run(&mut s, rm_ids(&["17941219991"], &["--no-confirm"])).unwrap();

    assert_eq!(
        s.out.lock().as_str(),
        format!(
            "Conversation(s) removed.\n\nThis command ran under `--no-persist`, {}.\n",
            "no files on disk were modified or removed".bold()
        )
    );
    assert!(dir.is_dir());
}

#[test]
fn the_status_says_why_a_conversation_does_not_load() {
    let corrupt = Err(ValidationError::UnreadableFile {
        file: ManagedFile::Metadata,
        source: FileError::Json(serde_json::from_str::<serde_json::Value>("not json").unwrap_err()),
    });

    assert_eq!(
        unloaded_status(&corrupt).as_deref(),
        Some("unable to load: metadata.json: invalid JSON at line 1, column 2")
    );
    assert_eq!(
        unloaded_status(&Ok(DirContents::Unstored)).as_deref(),
        Some("no conversation files")
    );
    assert_eq!(
        unloaded_status(&Ok(DirContents::Empty)).as_deref(),
        Some("empty conversation directory")
    );
}

// The state is read under the conversation's lock, so a conversation another
// process finished writing after this run's scan has no status to report.
#[test]
fn a_conversation_that_loads_has_no_status() {
    assert_eq!(unloaded_status(&Ok(DirContents::Stored)), None);
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
