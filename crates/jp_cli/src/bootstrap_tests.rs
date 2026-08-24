use std::str::FromStr as _;

use camino::Utf8PathBuf;
use camino_tempfile::tempdir;
use jp_workspace::{
    Id,
    session::{Session, SessionId, SessionSource},
    session_store::WorkspaceSessionStore,
};

use super::*;

/// Create a workspace root (a directory containing `.jp/`) under `base`.
fn make_workspace(base: &Utf8Path, name: &str) -> Utf8PathBuf {
    let root = base.join(name);
    std::fs::create_dir_all(root.join(DEFAULT_STORAGE_DIR)).unwrap();
    root
}

/// Create a workspace root whose storage directory carries a readable ID, so
/// liveness checks (`roots::is_live`) can match it.
fn make_workspace_with_id(base: &Utf8Path, name: &str, id: &str) -> Utf8PathBuf {
    let root = make_workspace(base, name);
    std::fs::write(root.join(DEFAULT_STORAGE_DIR).join(".id"), id).unwrap();
    root
}

/// A `TargetEnv` with explicit launch cwd, user-data directory, session, and
/// interactivity, so tests never touch the real process environment.
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

/// An `Env`-sourced session identity.
fn env_session() -> Session {
    Session {
        id: SessionId::new("42").expect("non-empty"),
        source: SessionSource::env("JP_SESSION"),
    }
}

/// Render an error with its full source chain, so assertions can match the
/// message of a wrapped `cmd::Error`.
fn error_message(error: &crate::Error) -> String {
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
fn cwd_inside_workspace_resolves_root_without_child_cwd() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "ws");

    let env = env_at(root.clone(), tmp.path(), None, false);
    let exec = resolve_from(&env, None).unwrap();

    assert_eq!(exec.root, root);
    assert_eq!(exec.launch_cwd, root);
    assert_eq!(exec.source, RootSource::Cwd);
    assert_eq!(exec.child_cwd(), None);
    assert_eq!(exec.config_cwd(), root);
}

#[test]
fn cwd_in_subdirectory_keeps_launch_cwd_and_config_cwd() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "ws");
    let subdir = root.join("crates").join("foo");
    std::fs::create_dir_all(&subdir).unwrap();

    let env = env_at(subdir.clone(), tmp.path(), None, false);
    let exec = resolve_from(&env, None).unwrap();

    assert_eq!(exec.root, root);
    assert_eq!(exec.launch_cwd, subdir);
    // Launched from inside the workspace: children inherit the process cwd,
    // and the subdirectory's `.jp.toml` chain keeps loading from the launch
    // cwd.
    assert_eq!(exec.child_cwd(), None);
    assert_eq!(exec.config_cwd(), subdir);
}

#[test]
fn cwd_outside_any_workspace_errors() {
    let tmp = tempdir().unwrap();
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let env = env_at(scratch, tmp.path(), None, false);
    let error = error_message(&resolve_from(&env, None).unwrap_err());

    assert!(
        error.contains("Could not locate workspace"),
        "unexpected error: {error}"
    );
}

#[test]
fn cwd_target_resolves_launch_directory_workspace() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "ws");

    // `-w cwd`: explicit, but resolves exactly like no target.
    let env = env_at(root.clone(), tmp.path(), None, false);
    let exec = resolve_from(&env, Some(&WorkspaceTarget::Cwd)).unwrap();

    assert_eq!(exec.root, root);
    assert_eq!(exec.source, RootSource::Cwd);
    assert_eq!(exec.child_cwd(), None);
}

#[test]
fn path_target_from_outside_runs_children_at_root() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "ws");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let target = WorkspaceTarget::Path(root.clone());
    let env = env_at(scratch.clone(), tmp.path(), None, false);
    let exec = resolve_from(&env, Some(&target)).unwrap();

    assert_eq!(exec.root, root);
    assert_eq!(exec.launch_cwd, scratch);
    assert_eq!(exec.source, RootSource::CliPath);
    // The root-as-working-directory invariant: children run as if launched
    // from the selected root, and config loads as if launched from there.
    assert_eq!(exec.child_cwd(), Some(root.as_path()));
    assert_eq!(exec.config_cwd(), root);
}

#[test]
fn path_target_to_subdirectory_resolves_containing_root() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "ws");
    let subdir = root.join("docs");
    std::fs::create_dir_all(&subdir).unwrap();
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let target = WorkspaceTarget::Path(subdir);
    let env = env_at(scratch, tmp.path(), None, false);
    let exec = resolve_from(&env, Some(&target)).unwrap();

    assert_eq!(exec.root, root);
    assert_eq!(exec.child_cwd(), Some(root.as_path()));
}

#[test]
fn path_target_into_own_workspace_leaves_child_cwd_unchanged() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "ws");
    let subdir = root.join("crates");
    std::fs::create_dir_all(&subdir).unwrap();

    // Standing inside the workspace and explicitly targeting it: children
    // inherit the process cwd, as if no target was given.
    let target = WorkspaceTarget::Path(root.clone());
    let env = env_at(subdir.clone(), tmp.path(), None, false);
    let exec = resolve_from(&env, Some(&target)).unwrap();

    assert_eq!(exec.root, root);
    assert_eq!(exec.source, RootSource::CliPath);
    assert_eq!(exec.child_cwd(), None);
    assert_eq!(exec.config_cwd(), subdir);
}

#[test]
fn path_target_from_different_workspace_runs_children_at_target_root() {
    let tmp = tempdir().unwrap();
    let target_root = make_workspace(tmp.path(), "target");
    let other_root = make_workspace(tmp.path(), "other");

    let target = WorkspaceTarget::Path(target_root.clone());
    let env = env_at(other_root.clone(), tmp.path(), None, false);
    let exec = resolve_from(&env, Some(&target)).unwrap();

    assert_eq!(exec.root, target_root);
    assert_eq!(exec.launch_cwd, other_root);
    assert_eq!(exec.child_cwd(), Some(target_root.as_path()));
    assert_eq!(exec.config_cwd(), target_root);
}

#[test]
fn nonexistent_path_target_errors() {
    let tmp = tempdir().unwrap();
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let target = WorkspaceTarget::Path(tmp.path().join("missing"));
    let env = env_at(scratch, tmp.path(), None, false);
    let error = error_message(&resolve_from(&env, Some(&target)).unwrap_err());

    assert!(
        error.contains("No workspace found"),
        "unexpected error: {error}"
    );
}

#[test]
fn session_active_workspace_wins_from_outside_when_interactive() {
    let tmp = tempdir().unwrap();
    let root = make_workspace_with_id(tmp.path(), "ws", "ws123");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let session = env_session();
    let env = env_at(scratch.clone(), tmp.path(), Some(&session), true);
    env.store
        .record_selection(&session, &Id::from_str("ws123").unwrap(), &root, Utc::now())
        .unwrap();

    let exec = resolve_from(&env, None).unwrap();

    assert_eq!(exec.root, root);
    assert_eq!(exec.launch_cwd, scratch);
    assert_eq!(exec.source, RootSource::SessionActive);
    // A from-anywhere run: children run at the selected root.
    assert_eq!(exec.child_cwd(), Some(root.as_path()));
    assert_eq!(exec.config_cwd(), root);
}

#[test]
fn cwd_matching_the_active_workspace_wins_without_prompt() {
    let tmp = tempdir().unwrap();
    let root = make_workspace_with_id(tmp.path(), "ws", "ws123");

    let session = env_session();
    let env = env_at(root.clone(), tmp.path(), Some(&session), true);
    env.store
        .record_selection(&session, &Id::from_str("ws123").unwrap(), &root, Utc::now())
        .unwrap();

    // Standing inside the active workspace is not a conflict: cwd resolution
    // applies without prompting.
    let exec = resolve_from(&env, None).unwrap();

    assert_eq!(exec.root, root);
    assert_eq!(exec.source, RootSource::Cwd);
    assert_eq!(exec.child_cwd(), None);
}

#[test]
fn sticky_session_keeps_its_active_workspace_over_a_different_cwd() {
    let tmp = tempdir().unwrap();
    let active = make_workspace_with_id(tmp.path(), "active", "ws123");
    let current = make_workspace_with_id(tmp.path(), "current", "ws456");

    let session = env_session();
    let env = env_at(current.clone(), tmp.path(), Some(&session), true);
    env.store
        .record_selection(
            &session,
            &Id::from_str("ws123").unwrap(),
            &active,
            Utc::now(),
        )
        .unwrap();
    env.store.set_sticky(&session, true).unwrap();

    // The persisted `A` choice: the active workspace wins without a prompt,
    // even though the cwd resolves to a different workspace.
    let exec = resolve_from(&env, None).unwrap();

    assert_eq!(exec.root, active);
    assert_eq!(exec.source, RootSource::SessionActive);
    // A from-anywhere run: children run at the selected root.
    assert_eq!(exec.child_cwd(), Some(active.as_path()));
    assert_eq!(exec.config_cwd(), active);
}

#[test]
fn dead_active_root_recovers_through_the_surviving_checkout() {
    let tmp = tempdir().unwrap();
    // The registry canonicalizes recorded paths; keep expectations literal.
    let base = tmp.path().canonicalize_utf8().unwrap();
    let dead = make_workspace_with_id(&base, "dead", "ws123");
    let survivor = make_workspace_with_id(&base, "survivor", "ws123");
    let scratch = base.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let session = env_session();
    let env = env_at(scratch, &base, Some(&session), true);
    env.store
        .record_selection(&session, &Id::from_str("ws123").unwrap(), &dead, Utc::now())
        .unwrap();
    jp_workspace::roots::upsert_root(&env.workspaces_dir.join("ws-ws123"), &survivor).unwrap();
    std::fs::remove_dir_all(&dead).unwrap();

    // One surviving checkout: recovery uses it directly and repairs the
    // session record (RFD 087's reprompt-on-missing-workspace).
    let exec = resolve_from(&env, None).unwrap();

    assert_eq!(exec.root, survivor);
    assert_eq!(exec.source, RootSource::SessionActive);
    assert_eq!(env.store.active(&session).unwrap().root, survivor);
}

#[test]
fn sticky_recovery_survives_a_different_cwd() {
    let tmp = tempdir().unwrap();
    let base = tmp.path().canonicalize_utf8().unwrap();
    let dead = make_workspace_with_id(&base, "dead", "ws123");
    let survivor = make_workspace_with_id(&base, "survivor", "ws123");
    let current = make_workspace_with_id(&base, "current", "ws456");

    let session = env_session();
    let env = env_at(current, &base, Some(&session), true);
    env.store
        .record_selection(&session, &Id::from_str("ws123").unwrap(), &dead, Utc::now())
        .unwrap();
    env.store.set_sticky(&session, true).unwrap();
    jp_workspace::roots::upsert_root(&env.workspaces_dir.join("ws-ws123"), &survivor).unwrap();
    std::fs::remove_dir_all(&dead).unwrap();

    // A sticky session whose recorded checkout died recovers through the
    // workspace's surviving checkout — the pin is not dropped (RFD 087).
    let exec = resolve_from(&env, None).unwrap();

    assert_eq!(exec.root, survivor);
    assert_eq!(exec.source, RootSource::SessionActive);
    let mapping = env.store.load(&session).unwrap();
    assert!(mapping.sticky);
    assert_eq!(mapping.history[0].root, survivor);
}

#[test]
fn cwd_sibling_of_a_dead_active_checkout_wins_without_prompt() {
    let tmp = tempdir().unwrap();
    let base = tmp.path().canonicalize_utf8().unwrap();
    let dead = make_workspace_with_id(&base, "dead", "ws123");
    let survivor = make_workspace_with_id(&base, "survivor", "ws123");

    let session = env_session();
    let env = env_at(survivor.clone(), &base, Some(&session), true);
    env.store
        .record_selection(&session, &Id::from_str("ws123").unwrap(), &dead, Utc::now())
        .unwrap();
    jp_workspace::roots::upsert_root(&env.workspaces_dir.join("ws-ws123"), &survivor).unwrap();
    std::fs::remove_dir_all(&dead).unwrap();

    // The recorded checkout is gone and the cwd is one of the workspace's
    // surviving checkouts: not a conflict — cwd wins without a prompt.
    let exec = resolve_from(&env, None).unwrap();

    assert_eq!(exec.root, survivor);
    assert_eq!(exec.source, RootSource::Cwd);
    assert_eq!(exec.child_cwd(), None);
}

#[test]
fn non_interactive_run_ignores_session_active_workspace() {
    let tmp = tempdir().unwrap();
    let root = make_workspace_with_id(tmp.path(), "ws", "ws123");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let session = env_session();
    let env = env_at(scratch, tmp.path(), Some(&session), false);
    env.store
        .record_selection(&session, &Id::from_str("ws123").unwrap(), &root, Utc::now())
        .unwrap();

    // Scripts never depend on hidden per-session state: outside a workspace
    // and without `--workspace`, a non-interactive run errors.
    let error = error_message(&resolve_from(&env, None).unwrap_err());

    assert!(
        error.contains("Could not locate workspace"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("--workspace"),
        "expected `--workspace` guidance: {error}"
    );
}

#[test]
fn dead_session_active_root_falls_through_to_error_without_candidates() {
    let tmp = tempdir().unwrap();
    let root = make_workspace_with_id(tmp.path(), "ws", "ws123");
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let session = env_session();
    let env = env_at(scratch, tmp.path(), Some(&session), true);
    env.store
        .record_selection(&session, &Id::from_str("ws123").unwrap(), &root, Utc::now())
        .unwrap();

    // The recorded root dies and no other workspace is known: the ladder
    // falls through the (empty) picker to the no-workspace error.
    std::fs::remove_dir_all(&root).unwrap();

    let error = error_message(&resolve_from(&env, None).unwrap_err());

    assert!(
        error.contains("Could not locate workspace"),
        "unexpected error: {error}"
    );
}

#[test]
fn interactive_run_without_session_identity_points_at_session_setup() {
    let tmp = tempdir().unwrap();
    let scratch = tmp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let env = env_at(scratch, tmp.path(), None, true);
    let error = error_message(&resolve_from(&env, None).unwrap_err());

    assert!(
        error.contains("no session identity"),
        "unexpected error: {error}"
    );
    assert!(error.contains("JP_SESSION"), "unexpected error: {error}");
}

#[test]
fn same_dir_tolerates_symlinked_spellings() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "ws");

    assert!(same_dir(&root, &root));
    assert!(!same_dir(&root, tmp.path()));

    #[cfg(unix)]
    {
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(same_dir(&root, &link));
    }
}

#[test]
fn symlinked_target_into_own_workspace_leaves_child_cwd_unchanged() {
    #[cfg(unix)]
    {
        let tmp = tempdir().unwrap();
        let root = make_workspace(tmp.path(), "ws");
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();

        // `-w` targeting a symlinked spelling of the workspace we are
        // standing in: same checkout, so children inherit the process cwd.
        let target = WorkspaceTarget::Path(link);
        let env = env_at(root.clone(), tmp.path(), None, false);
        let exec = resolve_from(&env, Some(&target)).unwrap();

        assert_eq!(exec.child_cwd(), None);
    }
}

/// A (env, session, active-root, cwd-root) fixture for conflict-choice tests:
/// `ws123` at `active/` is the recorded selection, `ws456` at `current/` is the
/// cwd workspace.
fn conflict_fixture(
    tmp: &Utf8Path,
    session: &Session,
) -> (TargetEnv<'static>, Utf8PathBuf, Utf8PathBuf) {
    let active = make_workspace_with_id(tmp, "active", "ws123");
    let current = make_workspace_with_id(tmp, "current", "ws456");

    let env = env_at(current.clone(), tmp, None, true);
    env.store
        .record_selection(
            session,
            &Id::from_str("ws123").unwrap(),
            &active,
            Utc::now(),
        )
        .unwrap();

    (env, active, current)
}

#[test]
fn conflict_choice_current_keeps_the_cwd_and_the_record() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let (env, active, current) = conflict_fixture(tmp.path(), &session);

    let (root, source) = apply_conflict_choice(
        &env,
        &session,
        ConflictChoice::Current,
        ActiveWorkspace::Live {
            root: active.clone(),
        },
        current.clone(),
    )
    .unwrap();

    assert_eq!(root, current);
    assert_eq!(source, RootSource::Cwd);
    // `c` is one-shot: the session record is untouched.
    let mapping = env.store.load(&session).unwrap();
    assert_eq!(mapping.history[0].root, active);
    assert!(!mapping.sticky);
}

#[test]
fn conflict_choice_current_and_select_records_the_cwd_workspace() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let (env, active, current) = conflict_fixture(tmp.path(), &session);

    let (root, source) = apply_conflict_choice(
        &env,
        &session,
        ConflictChoice::CurrentAndSelect,
        ActiveWorkspace::Live {
            root: active.clone(),
        },
        current.clone(),
    )
    .unwrap();

    assert_eq!(root, current);
    assert_eq!(source, RootSource::Cwd);
    // `C` records the cwd workspace as the new selection; the former active
    // workspace becomes the `s` target.
    let mapping = env.store.load(&session).unwrap();
    assert_eq!(mapping.history[0].workspace_id, "ws456");
    assert_eq!(mapping.history[0].root, current);
    assert_eq!(mapping.history[1].root, active);
    assert!(!mapping.sticky);
}

#[test]
fn conflict_choice_active_resolves_the_recorded_root_without_pinning() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let (env, active, current) = conflict_fixture(tmp.path(), &session);

    let (root, source) = apply_conflict_choice(
        &env,
        &session,
        ConflictChoice::Active,
        ActiveWorkspace::Live {
            root: active.clone(),
        },
        current,
    )
    .unwrap();

    assert_eq!(root, active);
    assert_eq!(source, RootSource::SessionActive);
    // `a` is one-shot: no sticky pin.
    assert!(!env.store.load(&session).unwrap().sticky);
}

#[test]
fn conflict_choice_active_and_stick_pins_the_session() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let (env, active, current) = conflict_fixture(tmp.path(), &session);

    let (root, source) = apply_conflict_choice(
        &env,
        &session,
        ConflictChoice::ActiveAndStick,
        ActiveWorkspace::Live {
            root: active.clone(),
        },
        current,
    )
    .unwrap();

    assert_eq!(root, active);
    assert_eq!(source, RootSource::SessionActive);
    // `A` persists on the session record.
    let mapping = env.store.load(&session).unwrap();
    assert!(mapping.sticky);
    assert_eq!(mapping.history[0].root, active);
}

#[test]
fn conflict_choice_quit_aborts_the_run() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let (env, active, current) = conflict_fixture(tmp.path(), &session);

    let error = error_message(
        &apply_conflict_choice(
            &env,
            &session,
            ConflictChoice::Quit,
            ActiveWorkspace::Live { root: active },
            current,
        )
        .unwrap_err(),
    );

    assert!(error.contains("Aborted"), "unexpected error: {error}");
}
