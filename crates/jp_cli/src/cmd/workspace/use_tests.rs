use std::str::FromStr as _;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use jp_printer::{OutputFormat, Printer};
use jp_workspace::{
    Id,
    session::{Session, SessionId, SessionSource},
    session_store::WorkspaceSessionStore,
};

use super::*;
use crate::cmd::workspace::target::TargetEnv;

fn env_at<'a>(
    launch_cwd: Utf8PathBuf,
    data_dir: &Utf8Path,
    session: Option<&'a Session>,
    interactive: bool,
) -> TargetEnv<'a> {
    TargetEnv {
        launch_cwd,
        workspaces_dir: data_dir.join(crate::USER_WORKSPACES_DIR),
        store: WorkspaceSessionStore::at_user_data_dir(data_dir),
        session,
        interactive,
    }
}

fn make_workspace(base: &Utf8Path, name: &str, id: &str) -> Utf8PathBuf {
    let root = base.join(name);
    std::fs::create_dir_all(root.join(crate::DEFAULT_STORAGE_DIR).as_std_path()).unwrap();
    std::fs::write(
        root.join(crate::DEFAULT_STORAGE_DIR)
            .join(".id")
            .as_std_path(),
        id,
    )
    .unwrap();
    root
}

fn env_session() -> Session {
    Session {
        id: SessionId::new("42").expect("non-empty"),
        source: SessionSource::env("JP_SESSION"),
    }
}

/// Flush the async printer, then read what reached the buffer.
fn stdout_of(printer: &Printer, buffer: &jp_printer::SharedBuffer) -> String {
    printer.flush();
    buffer.lock().clone()
}

/// Render an error with its full source chain via `Display`.
///
/// `Debug` would escape every backslash in Windows paths, breaking `contains()`
/// assertions on messages that list filesystem roots.
fn message_of(error: &crate::cmd::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        message.push_str(": ");
        message.push_str(&inner.to_string());
        source = inner.source();
    }
    message
}

#[test]
fn non_interactive_use_is_rejected() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), false);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);

    let error = Use { target: None }.run(&printer, &env).unwrap_err();
    assert!(
        message_of(&error).contains("interactive-only"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn use_without_a_session_identity_is_rejected() {
    let tmp = tempdir().unwrap();
    let env = env_at(tmp.path().to_owned(), tmp.path(), None, true);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);

    let error = Use { target: None }.run(&printer, &env).unwrap_err();
    assert!(
        message_of(&error).contains("No session identity"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn selecting_a_path_records_the_selection() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);
    let (printer, out, _err) = Printer::memory(OutputFormat::Text);

    Use {
        target: Some(WorkspaceTarget::Path(root.clone())),
    }
    .run(&printer, &env)
    .unwrap();

    let stdout = stdout_of(&printer, &out);
    assert!(
        stdout.contains("Session-active workspace set to"),
        "unexpected output: {stdout}"
    );

    let active = env.store.active(&session).expect("active entry");
    assert_eq!(active.workspace_id, "ws123");
    assert_eq!(active.root, root);
}

#[test]
fn reselecting_the_active_workspace_is_a_noop() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Path(root.clone())),
    }
    .run(&printer, &env)
    .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Path(root.clone())),
    }
    .run(&printer, &env)
    .unwrap();

    let stdout = stdout_of(&printer, &out);
    assert!(
        stdout.contains("Already the session-active workspace"),
        "unexpected output: {stdout}"
    );
}

#[test]
fn use_cwd_clears_the_selection() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Path(root)),
    }
    .run(&printer, &env)
    .unwrap();
    assert!(env.store.load(&session).is_some());

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Cwd),
    }
    .run(&printer, &env)
    .unwrap();

    let stdout = stdout_of(&printer, &out);
    assert!(stdout.contains("Cleared"), "unexpected output: {stdout}");
    assert!(env.store.load(&session).is_none());
}

#[test]
fn a_path_without_a_workspace_id_is_rejected() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);

    // A directory with no `.jp` storage anywhere up the tree: target
    // resolution finds no workspace at all.
    let plain = tmp.path().join("plain");
    std::fs::create_dir_all(plain.as_std_path()).unwrap();

    let error = Use {
        target: Some(WorkspaceTarget::Path(plain)),
    }
    .run(&printer, &env)
    .unwrap_err();
    assert!(
        message_of(&error).contains("No workspace found"),
        "unexpected error: {error:?}"
    );

    // A `.jp` directory without an `.id` file: a root is found, but it is
    // not a recognizable workspace, so nothing is recorded.
    let no_id = tmp.path().join("no-id");
    std::fs::create_dir_all(no_id.join(crate::DEFAULT_STORAGE_DIR).as_std_path()).unwrap();

    let error = Use {
        target: Some(WorkspaceTarget::Path(no_id)),
    }
    .run(&printer, &env)
    .unwrap_err();
    assert!(
        message_of(&error).contains("recognizable JP workspace"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn selections_are_scoped_to_the_session() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Path(root)),
    }
    .run(&printer, &env)
    .unwrap();

    // A different session sees no selection.
    let other = Session {
        id: SessionId::new("43").expect("non-empty"),
        source: SessionSource::env("JP_SESSION"),
    };
    assert!(env.store.load(&other).is_none());

    let active = env.store.active(&session).expect("active entry");
    assert_eq!(
        active.workspace_id,
        Id::from_str("ws123").unwrap().to_string()
    );
}
