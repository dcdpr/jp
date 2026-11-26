use std::{fs, process::Command, sync::mpsc, thread, time::Duration};

use camino_tempfile::Utf8TempDir;
use jp_test::env_var::EnvVarGuard;
use serial_test::serial;

use super::*;

fn writable() -> Vec<String> {
    vec!["docs/ticket".to_owned(), "docs/rfd".to_owned()]
}

#[test]
fn status_paths_include_both_sides_of_a_rename() {
    let status = "?? docs/ticket/0abc-new.md\0R  docs/ticket/0abc-b.md\0docs/ticket/0abc-a.md\0 M \
                  .jp/config.toml\0";

    assert_eq!(changed_paths(status), vec![
        "docs/ticket/0abc-new.md",
        "docs/ticket/0abc-b.md",
        "docs/ticket/0abc-a.md",
        ".jp/config.toml",
    ]);
}

#[test]
fn only_paths_under_a_writable_directory_are_allowed() {
    let changed = vec![
        "docs/ticket/0abc-new.md".to_owned(),
        "docs/ticket/.board.json".to_owned(),
        "docs/rfd".to_owned(),
        "docs/rfdx/evil.md".to_owned(),
        ".jp/conversations/1/events.json".to_owned(),
    ];

    assert_eq!(outside(&changed, &writable()), vec![
        "docs/rfdx/evil.md",
        ".jp/conversations/1/events.json"
    ]);
}

fn setup(dir: &Utf8Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> (Utf8TempDir, Utf8TempDir) {
    let origin = Utf8TempDir::new().unwrap();
    setup(origin.path(), &["init", "-q", "-b", "main"]);
    setup(origin.path(), &["config", "user.email", "test@example.com"]);
    setup(origin.path(), &["config", "user.name", "Test"]);
    fs::create_dir_all(origin.path().join("docs/ticket")).unwrap();
    fs::write(origin.path().join("docs/ticket/a.md"), "a\n").unwrap();
    setup(origin.path(), &["add", "."]);
    setup(origin.path(), &["commit", "-q", "-m", "first"]);

    let clone = Utf8TempDir::new().unwrap();
    setup(clone.path(), &["clone", "-q", origin.path().as_str(), "."]);

    (origin, clone)
}

fn commit_upstream(origin: &Utf8Path, file: &str) {
    fs::write(origin.join(file), "upstream\n").unwrap();
    setup(origin, &["add", "."]);
    setup(origin, &["commit", "-q", "-m", file]);
}

#[test]
#[serial(env_vars)]
fn a_clean_checkout_pulls_before_a_turn() {
    let _git_global = EnvVarGuard::set("GIT_CONFIG_GLOBAL", "");
    let _git_system = EnvVarGuard::set("GIT_CONFIG_SYSTEM", "");
    let (origin, clone) = repository();
    let mut checkout = GitCheckout::open(clone.path(), writable()).unwrap();
    commit_upstream(origin.path(), "docs/ticket/b.md");

    assert_eq!(checkout.begin(), Ok(None));
    assert!(clone.path().join("docs/ticket/b.md").exists());
    assert_eq!(checkout.end(), Ok(vec![]));
}

#[test]
#[serial(env_vars)]
fn a_checkout_with_changes_is_not_pulled() {
    let _git_global = EnvVarGuard::set("GIT_CONFIG_GLOBAL", "");
    let _git_system = EnvVarGuard::set("GIT_CONFIG_SYSTEM", "");
    let (origin, clone) = repository();
    let mut checkout = GitCheckout::open(clone.path(), writable()).unwrap();
    fs::write(clone.path().join("docs/ticket/new.md"), "filed\n").unwrap();
    commit_upstream(origin.path(), "docs/ticket/b.md");

    assert_eq!(checkout.is_clean(), Ok(false));
    assert_eq!(checkout.begin(), Ok(None));
    assert!(!clone.path().join("docs/ticket/b.md").exists());
    assert_eq!(checkout.end(), Ok(vec![]));

    assert_eq!(checkout.idle(), None);
    assert!(!clone.path().join("docs/ticket/b.md").exists());
}

#[test]
#[serial(env_vars)]
fn a_change_outside_the_writable_directories_is_reported() {
    let _git_global = EnvVarGuard::set("GIT_CONFIG_GLOBAL", "");
    let _git_system = EnvVarGuard::set("GIT_CONFIG_SYSTEM", "");
    let (_origin, clone) = repository();
    let mut checkout = GitCheckout::open(clone.path(), writable()).unwrap();

    checkout.begin().unwrap();
    fs::write(clone.path().join("README.md"), "changed\n").unwrap();

    assert_eq!(checkout.end(), Ok(vec!["README.md".to_owned()]));
    assert_eq!(checkout.stray(), Ok(vec!["README.md".to_owned()]));
}

#[test]
#[serial(env_vars)]
fn begin_waits_for_the_lock() {
    let _git_global = EnvVarGuard::set("GIT_CONFIG_GLOBAL", "");
    let _git_system = EnvVarGuard::set("GIT_CONFIG_SYSTEM", "");
    let (_origin, clone) = repository();
    let mut checkout = GitCheckout::open(clone.path(), writable()).unwrap();
    let held = checkout.lock().unwrap();

    let path = checkout.lock_path.clone();
    let (tx, rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.lock().unwrap();
        tx.send(()).unwrap();
    });

    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "the lock was taken while held"
    );
    drop(held);
    rx.recv_timeout(Duration::from_secs(5))
        .expect("the lock was never released");
    waiter.join().unwrap();

    assert_eq!(checkout.begin(), Ok(None));
    assert_eq!(checkout.end(), Ok(vec![]));
}

#[test]
#[serial(env_vars)]
fn a_directory_outside_git_is_refused() {
    let _git_global = EnvVarGuard::set("GIT_CONFIG_GLOBAL", "");
    let _git_system = EnvVarGuard::set("GIT_CONFIG_SYSTEM", "");
    let dir = Utf8TempDir::new().unwrap();

    let error = GitCheckout::open(dir.path(), writable()).unwrap_err();

    assert!(
        error.ends_with("run the bridge in one, or set `checkout.enabled = false`."),
        "{error}"
    );
}
