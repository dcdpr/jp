//! `take.sh`, the server half of `just chat-take`, run against a real git
//! repository.
//!
//! `flock` is replaced by a stub on `PATH`, since macOS has none; the lock
//! itself is the bridge's concern and is tested there.

#![cfg(unix)]

use std::{
    env, fs,
    os::unix::fs::PermissionsExt as _,
    process::{Command, Output},
};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use sha2::{Digest as _, Sha256};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/take.sh");

/// A bridge checkout with one commit, and a directory of stub commands that
/// goes first on `PATH`.
struct Server {
    repo: Utf8TempDir,
    bin: Utf8TempDir,
}

impl Server {
    fn new() -> Self {
        let repo = Utf8TempDir::new().unwrap();
        git(repo.path(), &["init", "--quiet"]);
        fs::write(repo.path().join("README.md"), "readme\n").unwrap();
        git(repo.path(), &["add", "README.md"]);
        git(repo.path(), &["commit", "--quiet", "-m", "init"]);

        let bin = Utf8TempDir::new().unwrap();
        stub(&bin.path().join("flock"), "#!/bin/sh\nexit 0\n");

        Self { repo, bin }
    }

    fn write(&self, path: &str) {
        let path = self.repo.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "ticket\n").unwrap();
    }

    fn exists(&self, path: &str) -> bool {
        self.repo.path().join(path).exists()
    }

    /// Make `git add` fail, as it does when the disk is full.
    fn break_git_add(&self) {
        stub(
            &self.bin.path().join("git"),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = add ]; then\n    echo 'error: insufficient space' >&2\n    \
                 exit 128\nfi\nexec {} \"$@\"\n",
                real_git()
            ),
        );
    }

    fn take(&self, args: &[&str]) -> Output {
        let path = format!("{}:{}", self.bin.path(), env::var("PATH").unwrap());

        Command::new("sh")
            .arg(SCRIPT)
            .args(args)
            .env("PATH", path)
            .output()
            .unwrap()
    }

    /// The base commit and hash `just chat-take` sends back with `reset`.
    fn snapshot(&self) -> (String, String) {
        let output = self.take(&["snapshot", self.repo.path().as_str()]);
        assert!(output.status.success(), "{output:?}");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let (base, diff) = stdout.split_once('\n').unwrap();
        let sum = format!("{:x}", Sha256::digest(diff.as_bytes()));

        (base.to_owned(), sum)
    }

    fn reset(&self, base: &str, sum: &str) -> Output {
        self.take(&["reset", self.repo.path().as_str(), base, sum])
    }
}

fn git(dir: &Utf8Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");

    String::from_utf8(output.stdout).unwrap()
}

fn stub(path: &Utf8Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// The `git` on `PATH`, which a stub hands every other command to.
fn real_git() -> Utf8PathBuf {
    env::var("PATH")
        .unwrap()
        .split(':')
        .map(|dir| Utf8PathBuf::from(dir).join("git"))
        .find(|path| path.is_file())
        .expect("git is on PATH")
}

#[test]
fn a_reset_after_an_unchanged_snapshot_cleans_the_checkout() {
    let server = Server::new();
    server.write("docs/ticket/a.md");
    let (base, sum) = server.snapshot();

    let output = server.reset(&base, &sum);

    assert!(output.status.success(), "{output:?}");
    assert!(!server.exists("docs/ticket/a.md"));
    assert_eq!(git(server.repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn a_change_after_the_snapshot_keeps_the_checkout() {
    let server = Server::new();
    server.write("docs/ticket/a.md");
    let (base, sum) = server.snapshot();
    server.write("docs/ticket/b.md");

    let output = server.reset(&base, &sum);

    assert_eq!(output.status.code(), Some(1));
    assert!(server.exists("docs/ticket/a.md"));
    assert!(server.exists("docs/ticket/b.md"));
}

/// A `git add` that fails leaves the snapshot's changes staged, whose diff
/// still matches the hash: a reset that went on would delete the ticket written
/// after the snapshot, which was never taken.
#[test]
fn a_failing_git_add_discards_nothing() {
    let server = Server::new();
    server.write("docs/ticket/a.md");
    let (base, sum) = server.snapshot();
    server.write("docs/ticket/b.md");
    server.break_git_add();

    let output = server.reset(&base, &sum);

    assert_eq!(output.status.code(), Some(128), "{output:?}");
    assert!(server.exists("docs/ticket/a.md"));
    assert!(server.exists("docs/ticket/b.md"));
}

#[test]
fn a_failing_git_add_fails_the_snapshot() {
    let server = Server::new();
    server.write("docs/ticket/a.md");
    server.break_git_add();

    let output = server.take(&["snapshot", server.repo.path().as_str()]);

    assert_eq!(output.status.code(), Some(128), "{output:?}");
}
