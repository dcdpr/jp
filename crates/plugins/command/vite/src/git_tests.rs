use camino_tempfile::Utf8TempDir;

use super::*;

/// Run `git` in `dir` isolated from global and system git config, so host
/// settings such as commit signing and hooks do not affect the test.
fn git(dir: &Utf8Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "")
        .env("GIT_CONFIG_SYSTEM", "")
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));

    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> Utf8TempDir {
    let top = Utf8TempDir::new().unwrap();
    git(top.path(), &["init", "--quiet", "--initial-branch=main"]);
    git(top.path(), &["config", "user.name", "Tester"]);
    git(top.path(), &["config", "user.email", "tester@example.com"]);

    top
}

#[test]
fn the_origin_remote_is_read_as_configured() {
    let top = repository();
    assert_eq!(remote(top.path()), Remote::default());

    git(top.path(), &[
        "remote",
        "add",
        "origin",
        "git@github.com:dcdpr/jp.git",
    ]);
    assert_eq!(remote(top.path()), Remote {
        origin: Some("git@github.com:dcdpr/jp.git".to_owned()),
        branch: None,
    });

    // `origin/HEAD` names the default branch, as a clone sets it.
    git(top.path(), &["commit", "--allow-empty", "-qm", "first"]);
    git(top.path(), &[
        "update-ref",
        "refs/remotes/origin/trunk",
        "HEAD",
    ]);
    git(top.path(), &[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/trunk",
    ]);
    assert_eq!(remote(top.path()).branch.as_deref(), Some("trunk"));
}
