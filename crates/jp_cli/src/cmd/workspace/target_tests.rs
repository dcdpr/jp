use std::str::FromStr as _;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use chrono::{TimeZone as _, Utc};
use jp_workspace::{
    roots::RootEntry,
    session::{Session, SessionId, SessionSource},
    session_store::WorkspaceSessionStore,
};

use super::*;

/// Build a [`TargetEnv`] rooted at `data_dir`, launched from `launch_cwd`.
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

/// Create a minimal on-disk workspace at `base/name` with the given ID.
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

/// Record `root` as a checkout of workspace `id` in the roots registry.
fn register(env: &TargetEnv<'_>, slug: &str, id: &str, root: &Utf8Path) {
    let silo = env.workspaces_dir.join(format!("{slug}-{id}"));
    jp_workspace::roots::upsert_root(&silo, root).unwrap();
}

/// Record `root` with an explicit `last_used` timestamp, for recency tests.
fn register_at(env: &TargetEnv<'_>, slug: &str, id: &str, root: &Utf8Path, last_used_secs: i64) {
    let dir = env
        .workspaces_dir
        .join(format!("{slug}-{id}"))
        .join("roots");
    std::fs::create_dir_all(dir.as_std_path()).unwrap();
    let entry = RootEntry {
        path: root.to_owned(),
        last_used: Utc.timestamp_opt(last_used_secs, 0).unwrap(),
    };
    std::fs::write(
        dir.join(format!("{slug}.json")).as_std_path(),
        serde_json::to_vec(&entry).unwrap(),
    )
    .unwrap();
}

fn env_session() -> Session {
    Session {
        id: SessionId::new("42").expect("non-empty"),
        source: SessionSource::env("JP_SESSION"),
    }
}

/// Render an error via `Debug`, which includes wrapped messages and sources.
fn message_of(error: &impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

#[test]
fn keywords_parse() {
    assert!(matches!(
        WorkspaceTarget::from_str("?").unwrap(),
        WorkspaceTarget::Picker
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("?s").unwrap(),
        WorkspaceTarget::SessionPicker
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("?session").unwrap(),
        WorkspaceTarget::SessionPicker
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("s").unwrap(),
        WorkspaceTarget::Session
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("session").unwrap(),
        WorkspaceTarget::Session
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("l").unwrap(),
        WorkspaceTarget::Latest
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("latest").unwrap(),
        WorkspaceTarget::Latest
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("cwd").unwrap(),
        WorkspaceTarget::Cwd
    ));
    assert!(matches!(
        WorkspaceTarget::from_str(".").unwrap(),
        WorkspaceTarget::Cwd
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("-").unwrap(),
        WorkspaceTarget::Stdin
    ));
    assert!(matches!(
        WorkspaceTarget::from_str("help").unwrap(),
        WorkspaceTarget::Help
    ));
    assert!(WorkspaceTarget::from_str("").is_err());
}

#[test]
fn id_path_and_fuzzy_parse() {
    // A well-formed workspace ID parses as an ID target.
    assert!(matches!(
        WorkspaceTarget::from_str("ws123").unwrap(),
        WorkspaceTarget::Id(id) if &*id == "ws123"
    ));

    // Free text that is not a valid ID parses as a fuzzy query.
    assert!(matches!(
        WorkspaceTarget::from_str("my project").unwrap(),
        WorkspaceTarget::Fuzzy(text) if text == "my project"
    ));

    // An existing path shadows everything else. Unit tests run from the
    // package root, where `src` exists.
    assert!(matches!(
        WorkspaceTarget::from_str("src").unwrap(),
        WorkspaceTarget::Path(path) if path == "src"
    ));
}

#[test]
fn stdin_id_parses_and_validates() {
    let id = stdin_id("ws123\n".as_bytes()).unwrap();
    assert_eq!(&*id, "ws123");

    let error = stdin_id("\n".as_bytes()).unwrap_err();
    assert!(
        message_of(&error).contains("No workspace ID on stdin"),
        "unexpected error: {error:?}"
    );

    assert!(stdin_id("definitely not an id\n".as_bytes()).is_err());
}

#[test]
fn id_with_no_live_roots_errors() {
    let tmp = tempdir().unwrap();
    let env = env_at(tmp.path().to_owned(), tmp.path(), None, false);

    let target = WorkspaceTarget::Id(Id::from_str("ws123").unwrap());
    let error = resolve(&target, &env).unwrap_err();

    assert!(
        message_of(&error).contains("no known live checkouts"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn id_with_one_live_root_resolves() {
    let tmp = tempdir().unwrap();
    let root = make_workspace(tmp.path(), "ws", "ws123");
    let env = env_at(tmp.path().to_owned(), tmp.path(), None, false);
    register(&env, "ws", "ws123", &root);

    let target = WorkspaceTarget::Id(Id::from_str("ws123").unwrap());
    let ResolvedTarget::Root(selected) = resolve(&target, &env).unwrap() else {
        panic!("expected a resolved root");
    };

    // The registry stores the canonicalized checkout path.
    assert_eq!(selected.root, root.canonicalize_utf8().unwrap());
    assert_eq!(selected.id, Some(Id::from_str("ws123").unwrap()));
}

#[test]
fn id_with_multiple_roots_is_ambiguous_non_interactively() {
    let tmp = tempdir().unwrap();
    let a = make_workspace(tmp.path(), "a", "ws123");
    let b = make_workspace(tmp.path(), "b", "ws123");
    let env = env_at(tmp.path().to_owned(), tmp.path(), None, false);
    register(&env, "ws", "ws123", &a);
    register(&env, "ws", "ws123", &b);

    let target = WorkspaceTarget::Id(Id::from_str("ws123").unwrap());
    let error = resolve(&target, &env).unwrap_err();

    let message = message_of(&error);
    assert!(
        message.contains("multiple checkouts"),
        "unexpected error: {message}"
    );
    // Both candidate roots are listed.
    assert!(message.contains(a.canonicalize_utf8().unwrap().as_str()));
    assert!(message.contains(b.canonicalize_utf8().unwrap().as_str()));
}

#[test]
fn session_and_picker_targets_are_interactive_only() {
    let tmp = tempdir().unwrap();
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), false);

    for target in [
        WorkspaceTarget::Session,
        WorkspaceTarget::SessionPicker,
        WorkspaceTarget::Picker,
        WorkspaceTarget::Fuzzy("anything".to_owned()),
    ] {
        let error = resolve(&target, &env).unwrap_err();
        assert!(
            message_of(&error).contains("interactive-only"),
            "target {target:?}: unexpected error: {error:?}"
        );
    }
}

#[test]
fn session_target_requires_a_session_identity() {
    let tmp = tempdir().unwrap();
    let env = env_at(tmp.path().to_owned(), tmp.path(), None, true);

    let error = resolve(&WorkspaceTarget::Session, &env).unwrap_err();
    assert!(
        message_of(&error).contains("No session identity"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn session_target_resolves_the_previous_checkout() {
    let tmp = tempdir().unwrap();
    let a = make_workspace(tmp.path(), "a", "aaa11");
    let b = make_workspace(tmp.path(), "b", "bbb22");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);

    env.store
        .record_selection(&session, &Id::from_str("aaa11").unwrap(), &a, Utc::now())
        .unwrap();
    env.store
        .record_selection(&session, &Id::from_str("bbb22").unwrap(), &b, Utc::now())
        .unwrap();

    // `s` is `cd -`: the previously active checkout, not the current one.
    let ResolvedTarget::Root(selected) = resolve(&WorkspaceTarget::Session, &env).unwrap() else {
        panic!("expected a resolved root");
    };
    assert_eq!(selected.root, a);
    assert_eq!(selected.id, Some(Id::from_str("aaa11").unwrap()));
}

#[test]
fn session_target_errors_without_a_previous_entry() {
    let tmp = tempdir().unwrap();
    let a = make_workspace(tmp.path(), "a", "aaa11");
    let session = env_session();
    let env = env_at(tmp.path().to_owned(), tmp.path(), Some(&session), true);

    env.store
        .record_selection(&session, &Id::from_str("aaa11").unwrap(), &a, Utc::now())
        .unwrap();

    let error = resolve(&WorkspaceTarget::Session, &env).unwrap_err();
    assert!(
        message_of(&error).contains("No previously active workspace"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn latest_resolves_the_newest_live_checkout() {
    let tmp = tempdir().unwrap();
    let a = make_workspace(tmp.path(), "a", "aaa11");
    let b = make_workspace(tmp.path(), "b", "bbb22");
    let env = env_at(tmp.path().to_owned(), tmp.path(), None, false);
    register_at(&env, "a", "aaa11", &a, 1_000);
    register_at(&env, "b", "bbb22", &b, 2_000);

    let ResolvedTarget::Root(selected) = resolve(&WorkspaceTarget::Latest, &env).unwrap() else {
        panic!("expected a resolved root");
    };
    assert_eq!(selected.root, b);
    assert_eq!(selected.id, Some(Id::from_str("bbb22").unwrap()));
}

#[test]
fn latest_errors_with_an_empty_registry() {
    let tmp = tempdir().unwrap();
    let env = env_at(tmp.path().to_owned(), tmp.path(), None, false);

    let error = resolve(&WorkspaceTarget::Latest, &env).unwrap_err();
    assert!(
        message_of(&error).contains("No known workspaces"),
        "unexpected error: {error:?}"
    );
}
