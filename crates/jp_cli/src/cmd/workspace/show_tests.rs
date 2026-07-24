use std::str::FromStr as _;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use chrono::Utc;
use jp_conversation::ConversationId;
use jp_printer::{OutputFormat, Printer};
use jp_workspace::{
    session::{Session, SessionId, SessionSource},
    session_store::WorkspaceSessionStore,
};

use super::*;

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
fn no_selection_is_a_first_class_outcome() {
    let tmp = tempdir().unwrap();
    let env = env_at(tmp.path().to_owned(), tmp.path(), None);
    let (printer, out, _err) = Printer::memory(OutputFormat::Text);

    Show { target: None }.run(&printer, &env, false).unwrap();

    let stdout = stdout_of(&printer, &out);
    assert!(
        stdout.contains("No workspace selected"),
        "unexpected output: {stdout}"
    );
}

#[test]
fn explicit_id_reports_missing_live_checkouts() {
    let tmp = tempdir().unwrap();
    let env = env_at(tmp.path().to_owned(), tmp.path(), None);
    let (printer, out, _err) = Printer::memory(OutputFormat::Text);

    Show {
        target: Some(WorkspaceTarget::Id(Id::from_str("ws123").unwrap())),
    }
    .run(&printer, &env, false)
    .unwrap();

    let stdout = stdout_of(&printer, &out);
    assert!(stdout.contains("ws123"), "unexpected output: {stdout}");
    assert!(
        stdout.contains("(no live checkouts)"),
        "unexpected output: {stdout}"
    );
    assert!(
        stdout.contains("explicit target"),
        "unexpected output: {stdout}"
    );
}

#[test]
fn ambiguous_fuzzy_match_errors_with_candidates() {
    let tmp = tempdir().unwrap();
    let env = env_at(tmp.path().to_owned(), tmp.path(), None);

    // Two known workspaces whose slugs share a prefix. No live checkouts are
    // needed: `known_workspaces` lists them regardless.
    std::fs::create_dir_all(env.workspaces_dir.join("alpha-one-aaa11").as_std_path()).unwrap();
    std::fs::create_dir_all(env.workspaces_dir.join("alpha-two-bbb22").as_std_path()).unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let error = Show {
        target: Some(WorkspaceTarget::Fuzzy("alpha".to_owned())),
    }
    .run(&printer, &env, false)
    .unwrap_err();

    let message = format!("{error:?}");
    assert!(
        message.contains("matches multiple workspaces"),
        "unexpected error: {message}"
    );
}

#[test]
fn conversation_count_unions_sibling_checkout_scans() {
    let tmp = tempdir().unwrap();

    // Two live checkouts of the same workspace ID. The most recently used one
    // pays the full workspace load; its sibling is only directory-scanned.
    let first = make_workspace(tmp.path(), "first", "ws123");
    let second = make_workspace(tmp.path(), "second", "ws123");

    let env = env_at(tmp.path().to_owned(), tmp.path(), None);
    let user_dir = env.workspaces_dir.join("proj-ws123");
    let id = Id::from_str("ws123").unwrap();
    // `first` is upserted last, so it is the most recently used checkout and
    // takes the full load; `second` stays a scanned sibling. A full load
    // would sanitize the bare (event-less) directory below away — the scan
    // must not, since it merely lists projected IDs.
    jp_workspace::roots::upsert_root(&user_dir, &second).unwrap();
    jp_workspace::roots::upsert_root(&user_dir, &first).unwrap();

    // A checkout-only conversation in the *second* root: present only as a
    // projection directory, invisible to the first root's full load.
    let conversation = ConversationId::from_str("jp-c17636257521").unwrap();
    std::fs::create_dir_all(
        second
            .join(crate::DEFAULT_STORAGE_DIR)
            .join("conversations")
            .join(conversation.to_dirname(None))
            .as_std_path(),
    )
    .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Json);
    Show {
        target: Some(WorkspaceTarget::Id(id)),
    }
    .run(&printer, &env, false)
    .unwrap();

    let stdout = stdout_of(&printer, &out);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        json["conversations"],
        serde_json::json!(1),
        "checkout-only conversation should be counted: {stdout}"
    );
    assert_eq!(
        json["checkouts"].as_array().map(Vec::len),
        Some(2),
        "both live checkouts should be listed: {stdout}"
    );
}

#[test]
fn session_active_subject_notes_cwd_precedence() {
    let tmp = tempdir().unwrap();
    // The cwd resolves to a live workspace...
    let cwd_root = make_workspace(tmp.path(), "here", "ccc33");
    // ...while the session-active workspace points at a removed checkout.
    let session = env_session();
    let env = env_at(cwd_root.clone(), tmp.path(), Some(&session));
    env.store
        .record_selection(
            &session,
            &Id::from_str("aaa11").unwrap(),
            &tmp.path().join("gone"),
            Utc::now(),
        )
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    Show { target: None }.run(&printer, &env, false).unwrap();

    let stdout = stdout_of(&printer, &out);
    // The subject is the session-active workspace, not the cwd one.
    assert!(stdout.contains("aaa11"), "unexpected output: {stdout}");
    assert!(
        stdout.contains("session-active"),
        "unexpected output: {stdout}"
    );
    // Session-level sticky state renders for the active subject.
    assert!(stdout.contains("Sticky"), "unexpected output: {stdout}");
    // The cwd-vs-active tension is surfaced: without a sticky pin, commands
    // run here prompt between the two (RFD 087).
    assert!(
        stdout.contains("prompt between"),
        "unexpected output: {stdout}"
    );
}
