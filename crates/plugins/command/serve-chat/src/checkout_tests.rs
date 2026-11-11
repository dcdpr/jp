use std::{fs, sync::mpsc, thread, time::Duration};

use camino_tempfile::Utf8TempDir;

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

fn repository() -> (Utf8TempDir, Utf8TempDir) {
    let origin = Utf8TempDir::new().unwrap();
    let run = |dir: &Utf8Path, args: &[&str]| {
        git(dir, args).unwrap_or_else(|error| panic!("git {args:?}: {error}"));
    };

    run(origin.path(), &["init", "-q", "-b", "main"]);
    run(origin.path(), &["config", "user.email", "test@example.com"]);
    run(origin.path(), &["config", "user.name", "Test"]);
    fs::create_dir_all(origin.path().join("docs/ticket")).unwrap();
    fs::write(origin.path().join("docs/ticket/a.md"), "a\n").unwrap();
    run(origin.path(), &["add", "."]);
    run(origin.path(), &["commit", "-q", "-m", "first"]);

    let clone = Utf8TempDir::new().unwrap();
    run(clone.path(), &["clone", "-q", origin.path().as_str(), "."]);

    (origin, clone)
}

fn commit_upstream(origin: &Utf8Path, file: &str) {
    fs::write(origin.join(file), "upstream\n").unwrap();
    git(origin, &["add", "."]).unwrap();
    git(origin, &["commit", "-q", "-m", file]).unwrap();
}

#[test]
fn a_clean_checkout_pulls_before_a_turn() {
    let (origin, clone) = repository();
    let mut checkout = GitCheckout::open(clone.path(), writable()).unwrap();
    commit_upstream(origin.path(), "docs/ticket/b.md");

    assert_eq!(checkout.begin(), Ok(None));
    assert!(clone.path().join("docs/ticket/b.md").exists());
    assert_eq!(checkout.end(), Ok(vec![]));
}

#[test]
fn a_checkout_with_changes_is_not_pulled() {
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
fn a_change_outside_the_writable_directories_is_reported() {
    let (_origin, clone) = repository();
    let mut checkout = GitCheckout::open(clone.path(), writable()).unwrap();

    checkout.begin().unwrap();
    fs::write(clone.path().join("README.md"), "changed\n").unwrap();

    assert_eq!(checkout.end(), Ok(vec!["README.md".to_owned()]));
    assert_eq!(checkout.stray(), Ok(vec!["README.md".to_owned()]));
}

#[test]
fn begin_waits_for_the_lock() {
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
fn a_directory_outside_git_is_refused() {
    let dir = Utf8TempDir::new().unwrap();

    let error = GitCheckout::open(dir.path(), writable()).unwrap_err();

    assert!(
        error.ends_with("run the bridge in one, or set `checkout.enabled = false`."),
        "{error}"
    );
}
