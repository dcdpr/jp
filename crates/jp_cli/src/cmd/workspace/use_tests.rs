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

    let error = Use {
        target: None,
        always: false,
    }
    .run(&printer, &env)
    .unwrap_err();
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

    let error = Use {
        target: None,
        always: false,
    }
    .run(&printer, &env)
    .unwrap_err();
    assert!(
        message_of(&error).contains("No session identity"),
        "unexpected error: {error:?}"
    );
}

// Without `--always` the session follows the cwd again as soon as the user
// stands in another workspace, which is what makes the conflict prompt fire.
#[test]
fn selecting_a_workspace_is_not_sticky_by_default() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);

    Use {
        target: Some(WorkspaceTarget::Path(root)),
        always: false,
    }
    .run(&printer, &env)
    .unwrap();

    assert!(!env.store.load(&session).expect("mapping").sticky);
}

// `--always` is the up-front form of the conflict prompt's `A`: the selection
// outranks the cwd from here on, stated once rather than extracted from
// someone dismissing a prompt.
#[test]
fn always_keeps_the_session_on_the_selection() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);
    let (printer, out, _err) = Printer::memory(OutputFormat::Text);

    Use {
        target: Some(WorkspaceTarget::Path(root.clone())),
        always: true,
    }
    .run(&printer, &env)
    .unwrap();

    assert!(env.store.load(&session).expect("mapping").sticky);

    let stdout = stdout_of(&printer, &out);
    assert!(
        stdout.contains("always"),
        "the sticky flag must be reported: {stdout}"
    );
}

// A later plain `use` states the whole intent for the session, so the sticky
// flag it does not ask for is one it takes away.
#[test]
fn selecting_again_without_always_releases_the_flag() {
    let tmp = tempdir().unwrap();
    let first = make_workspace(tmp.path(), "first", "ws123");
    let second = make_workspace(tmp.path(), "second", "ws456");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);

    Use {
        target: Some(WorkspaceTarget::Path(first)),
        always: true,
    }
    .run(&printer, &env)
    .unwrap();
    assert!(env.store.load(&session).expect("mapping").sticky);

    Use {
        target: Some(WorkspaceTarget::Path(second)),
        always: false,
    }
    .run(&printer, &env)
    .unwrap();
    assert!(!env.store.load(&session).expect("mapping").sticky);
}

// Re-selecting the workspace that is already active takes the early-return
// path, where the release is the only thing that happened: without the notice
// the run reports a no-op while the record changed underneath.
#[test]
fn reselecting_the_active_workspace_reports_the_release() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Path(root.clone())),
        always: true,
    }
    .run(&printer, &env)
    .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Path(root)),
        always: false,
    }
    .run(&printer, &env)
    .unwrap();

    assert!(!env.store.load(&session).expect("mapping").sticky);

    let stdout = stdout_of(&printer, &out);
    assert!(
        stdout.contains("Already the session-active workspace"),
        "unexpected output: {stdout}"
    );
    assert!(
        stdout.contains("no longer always"),
        "the release must be reported: {stdout}"
    );
}

// `cwd` drops the record entirely; there would be nothing left to keep active.
#[test]
fn always_with_the_cwd_target_is_rejected() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);

    let error = Use {
        target: Some(WorkspaceTarget::Cwd),
        always: true,
    }
    .run(&printer, &env)
    .unwrap_err();

    assert!(
        message_of(&error).contains("--always"),
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
        always: false,
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
fn selecting_a_workspace_registers_its_checkout() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);

    // A checkout no workspace-loading command has run in: nothing knows it
    // yet.
    let id = Id::from_str("ws123").unwrap();
    assert!(
        jp_workspace::roots::resolve_live_roots(
            &env.workspaces_dir,
            &id,
            crate::DEFAULT_STORAGE_DIR
        )
        .is_empty()
    );

    Use {
        target: Some(WorkspaceTarget::Path(root.clone())),
        always: false,
    }
    .run(&printer, &env)
    .unwrap();

    // Selecting it makes it reachable by ID from anywhere — and keeps the
    // end-of-run cleanup pass, which prunes selections nothing vouches for,
    // from dropping the record just written.
    let roots = jp_workspace::roots::resolve_live_roots(
        &env.workspaces_dir,
        &id,
        crate::DEFAULT_STORAGE_DIR,
    );
    assert_eq!(roots.len(), 1, "the selected checkout is registered");
    assert_eq!(roots[0].path, root.canonicalize_utf8().unwrap());
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
        always: false,
    }
    .run(&printer, &env)
    .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Path(root.clone())),
        always: false,
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
        always: false,
    }
    .run(&printer, &env)
    .unwrap();
    assert!(env.store.load(&session).is_some());

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Use {
        target: Some(WorkspaceTarget::Cwd),
        always: false,
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
        always: false,
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
        always: false,
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
        always: false,
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
