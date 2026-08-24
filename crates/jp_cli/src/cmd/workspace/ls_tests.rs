use std::str::FromStr as _;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use chrono::Utc;
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
) -> TargetEnv<'a> {
    TargetEnv {
        launch_cwd,
        workspaces_dir: data_dir.join(crate::USER_WORKSPACES_DIR),
        store: WorkspaceSessionStore::at_user_data_dir(data_dir),
        session,
        interactive: false,
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

fn register(env: &TargetEnv<'_>, slug: &str, id: &str, root: &Utf8Path) {
    let silo = env.workspaces_dir.join(format!("{slug}-{id}"));
    jp_workspace::roots::upsert_root(&silo, root).unwrap();
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

#[test]
fn empty_registry_prints_a_hint() {
    let tmp = tempdir().unwrap();
    let env = env_at(tmp.path().to_owned(), tmp.path(), None);
    let (printer, out, _err) = Printer::memory(OutputFormat::Text);

    Ls {}.run(&printer, &env).unwrap();

    let stdout = stdout_of(&printer, &out);
    assert!(
        stdout.contains("No known workspaces"),
        "unexpected output: {stdout}"
    );
}

#[test]
fn lists_workspaces_with_their_checkouts() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let env = env_at(tmp.path().to_owned(), tmp.path(), None);
    register(&env, "proj", "ws123", &root);

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Ls {}.run(&printer, &env).unwrap();

    let stdout = stdout_of(&printer, &out);
    assert!(stdout.contains("ws123"), "unexpected output: {stdout}");
    assert!(stdout.contains("proj"), "unexpected output: {stdout}");
    assert!(
        stdout.contains(root.canonicalize_utf8().unwrap().as_str()),
        "unexpected output: {stdout}"
    );
}

#[test]
fn marks_the_session_active_checkout() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session));
    register(&env, "proj", "ws123", &root);

    // The registry stores the canonical path; record the same value so the
    // active marker matches the listed checkout.
    let canonical = root.canonicalize_utf8().unwrap();
    env.store
        .record_selection(
            &session,
            &Id::from_str("ws123").unwrap(),
            &canonical,
            Utc::now(),
        )
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Ls {}.run(&printer, &env).unwrap();

    let stdout = stdout_of(&printer, &out);
    let marked = stdout
        .lines()
        .any(|line| line.contains('*') && line.contains(canonical.as_str()));
    assert!(marked, "no active marker in output: {stdout}");
}

#[test]
fn json_format_nests_checkouts_per_workspace() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "proj", "ws123");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session));
    register(&env, "proj", "ws123", &root);

    let canonical = root.canonicalize_utf8().unwrap();
    env.store
        .record_selection(
            &session,
            &Id::from_str("ws123").unwrap(),
            &canonical,
            Utc::now(),
        )
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Json);
    Ls {}.run(&printer, &env).unwrap();

    let stdout = stdout_of(&printer, &out);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let workspaces = json.as_array().expect("list output is a JSON array");
    assert_eq!(workspaces.len(), 1, "one object per workspace: {stdout}");

    // One self-contained object per workspace: identity at the top,
    // checkouts nested — not one row-shaped object per checkout, and no
    // display-column keys.
    let workspace = &workspaces[0];
    assert_eq!(workspace["id"], serde_json::json!("ws123"));
    assert_eq!(workspace["slug"], serde_json::json!("proj"));

    let checkouts = workspace["checkouts"]
        .as_array()
        .expect("checkouts is a JSON array");
    assert_eq!(checkouts.len(), 1, "one entry per live checkout: {stdout}");

    let checkout = &checkouts[0];
    assert_eq!(checkout["path"], serde_json::json!(canonical.as_str()));
    assert_eq!(
        checkout["active"],
        serde_json::json!(true),
        "session-active checkout flags `active`: {stdout}"
    );
    assert!(
        checkout["last_used"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "last-used timestamp should be present: {stdout}"
    );
}
