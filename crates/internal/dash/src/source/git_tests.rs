use std::fs;

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use jp_test::env_var::EnvVarGuard;
use serial_test::serial;

use super::*;

fn run(dir: &Utf8Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn commit(work: &Utf8Path, file: &str, text: &str) -> String {
    fs::write(work.join(file), text).unwrap();
    run(work, &["add", file]);
    run(work, &["commit", "--quiet", "-m", file]);
    run(work, &["rev-parse", "HEAD"])
}

/// Initialize a bare `origin` remote and a `work` clone with `main` and a
/// two-branch stack (`b1` on `main`, `b2` on `b1`), returning `(work, remote,
/// b1_sha, b2_sha)`.
fn stack(tmp: &Utf8TempDir, b1_text: &str) -> (Utf8PathBuf, Utf8PathBuf, String, String) {
    let remote = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    run(tmp.path(), &[
        "init",
        "--quiet",
        "--bare",
        "-b",
        "main",
        remote.as_str(),
    ]);
    run(tmp.path(), &[
        "init",
        "--quiet",
        "-b",
        "main",
        work.as_str(),
    ]);
    run(&work, &["config", "user.name", "Test"]);
    run(&work, &["config", "user.email", "test@example.com"]);
    run(&work, &["remote", "add", "origin", remote.as_str()]);

    commit(&work, "shared.txt", "base\n");
    run(&work, &["push", "--quiet", "origin", "main"]);

    run(&work, &["switch", "--quiet", "-c", "b1"]);
    let b1 = commit(&work, "shared.txt", b1_text);
    run(&work, &["switch", "--quiet", "-c", "b2"]);
    let b2 = commit(&work, "b2.txt", "two\n");
    run(&work, &["push", "--quiet", "origin", "b1", "b2"]);

    run(&work, &["switch", "--quiet", "main"]);
    (work, remote, b1, b2)
}

fn branches(b1: &str, b2: &str) -> Vec<(String, String)> {
    vec![
        ("b1".to_owned(), b1.to_owned()),
        ("b2".to_owned(), b2.to_owned()),
    ]
}

fn is_ancestor(dir: &Utf8Path, ancestor: &str, of: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["merge-base", "--is-ancestor", ancestor, of])
        .status()
        .unwrap()
        .success()
}

#[test]
#[serial(env_vars)]
fn test_rebase_stack_moves_every_branch_onto_the_new_base() {
    let _git_global = EnvVarGuard::set("GIT_CONFIG_GLOBAL", "");
    let _git_system = EnvVarGuard::set("GIT_CONFIG_SYSTEM", "");
    let tmp = Utf8TempDir::new().unwrap();
    let (work, remote, b1, b2) = stack(&tmp, "base\none\n");
    commit(&work, "main.txt", "later\n");
    run(&work, &["push", "--quiet", "origin", "main"]);

    let rebased = rebase_stack(&work, "origin", "main", &branches(&b1, &b2), None).unwrap();

    assert_eq!(rebased, Rebased::Pushed);
    assert!(is_ancestor(&remote, "main", "b1"));
    assert!(is_ancestor(&remote, "b1", "b2"));
    assert_eq!(
        run(&remote, &["ls-tree", "-r", "--name-only", "b2"]),
        "b2.txt\nmain.txt\nshared.txt"
    );
    // The scratch worktree is gone again.
    assert_eq!(run(&work, &["worktree", "list"]).lines().count(), 1);
}

#[test]
#[serial(env_vars)]
fn test_rebase_stack_pushes_nothing_once_a_branch_moved() {
    let _git_global = EnvVarGuard::set("GIT_CONFIG_GLOBAL", "");
    let _git_system = EnvVarGuard::set("GIT_CONFIG_SYSTEM", "");
    let tmp = Utf8TempDir::new().unwrap();
    let (work, remote, b1, b2) = stack(&tmp, "base\none\n");
    commit(&work, "main.txt", "later\n");
    run(&work, &["push", "--quiet", "origin", "main"]);

    // Someone pushed to `b2` after GitHub reported `b2`'s head.
    run(&work, &["switch", "--quiet", "b2"]);
    commit(&work, "b2.txt", "three\n");
    run(&work, &["push", "--quiet", "origin", "b2"]);
    let pushed = run(&remote, &["rev-parse", "b2"]);

    let rebased = rebase_stack(&work, "origin", "main", &branches(&b1, &b2), None).unwrap();

    assert_eq!(rebased, Rebased::Moved);
    assert_eq!(run(&remote, &["rev-parse", "b1"]), b1);
    assert_eq!(run(&remote, &["rev-parse", "b2"]), pushed);
}

#[test]
#[serial(env_vars)]
fn test_rebase_stack_reports_conflicts_and_leaves_the_branches_alone() {
    let _git_global = EnvVarGuard::set("GIT_CONFIG_GLOBAL", "");
    let _git_system = EnvVarGuard::set("GIT_CONFIG_SYSTEM", "");
    let tmp = Utf8TempDir::new().unwrap();
    let (work, remote, b1, b2) = stack(&tmp, "base\none\n");
    // `main` changes the line `b1` changed.
    commit(&work, "shared.txt", "base\nother\n");
    run(&work, &["push", "--quiet", "origin", "main"]);

    let rebased = rebase_stack(&work, "origin", "main", &branches(&b1, &b2), None).unwrap();

    assert_eq!(rebased, Rebased::Conflicts);
    assert_eq!(run(&remote, &["rev-parse", "b1"]), b1);
    assert_eq!(run(&remote, &["rev-parse", "b2"]), b2);
    assert_eq!(run(&work, &["worktree", "list"]).lines().count(), 1);
}

#[test]
fn test_points_at_matches_https_and_ssh_remotes() {
    assert!(points_at("https://github.com/dcdpr/jp.git", "dcdpr/jp"));
    assert!(points_at("https://github.com/dcdpr/jp", "dcdpr/jp"));
    assert!(points_at("git@github.com:dcdpr/jp.git", "dcdpr/jp"));
    assert!(points_at("git@github.com:DCDPR/JP.git", "dcdpr/jp"));
    assert!(points_at("ssh://git@github.com/dcdpr/jp.git/", "dcdpr/jp"));

    assert!(!points_at("git@github.com:octo/jp.git", "dcdpr/jp"));
    assert!(!points_at("git@github.com:dcdpr/jp-tools.git", "dcdpr/jp"));
    assert!(!points_at("git@github.com:xdcdpr/jp.git", "dcdpr/jp"));
}

#[test]
fn test_parse_keeps_branches_and_detached_heads() {
    let porcelain = "\
worktree /repo/.bare
bare

worktree /repo/main
HEAD b9de1cfbe88066775d2aa6bdafa29b4d75f6d3b1
branch refs/heads/main

worktree /repo/review
HEAD 6d55a720113ffb7692141f5288883f5e0cec8b30
detached

worktree /repo/gone
HEAD 58f36525aa529aefab357438862ee792878609ae
branch refs/heads/gone
prunable gitdir file points to non-existent location

worktree /repo/locked
HEAD 2017653d43750065972b46171cef492575a62b72
branch refs/heads/feature/locked
locked
";

    assert_eq!(parse(porcelain), vec![
        Worktree {
            path: Utf8PathBuf::from("/repo/main"),
            branch: Some("main".to_owned()),
        },
        Worktree {
            path: Utf8PathBuf::from("/repo/review"),
            branch: None,
        },
        Worktree {
            path: Utf8PathBuf::from("/repo/locked"),
            branch: Some("feature/locked".to_owned()),
        },
    ]);
}

#[test]
fn test_parse_empty_output() {
    assert_eq!(parse(""), vec![]);
}
