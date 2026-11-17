//! Git worktree discovery and branch operations.
//!
//! Every command that may contact a remote runs with stdin closed and
//! interactive prompts disabled so a credential prompt cannot draw over the
//! dashboard TUI or block a background thread indefinitely.

use std::{
    env,
    process::{Command, Stdio},
    sync::{Mutex, PoisonError},
};

use camino::{Utf8Path, Utf8PathBuf};

use super::{Error, canonical, stdout};
use crate::model::Worktree;

/// Serializes default-branch updates so periodic background fetches and
/// worktree-creation fetches do not update the same refs concurrently.
static UPDATING: Mutex<()> = Mutex::new(());

/// Configure a `git` or `gh` `command` running in `dir` to fail instead of
/// prompting for credentials.
///
/// Disables Git and Git Credential Manager prompts and appends `-o
/// BatchMode=yes` to the configured SSH command.
pub fn noninteractive<'a>(command: &'a mut Command, dir: &Utf8Path) -> &'a mut Command {
    let ssh = env::var("GIT_SSH_COMMAND")
        .ok()
        .filter(|ssh| !ssh.trim().is_empty())
        .or_else(|| {
            stdout(
                "git",
                Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .args(["config", "core.sshCommand"]),
            )
            .ok()
            .map(|ssh| ssh.trim().to_owned())
            .filter(|ssh| !ssh.is_empty())
        })
        .unwrap_or_else(|| "ssh".to_owned());

    command
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_SSH_COMMAND", format!("{ssh} -o BatchMode=yes"))
}

/// List the worktrees of the repository containing `dir`.
pub fn worktrees(dir: &Utf8Path) -> Result<Vec<Worktree>, Error> {
    let porcelain = stdout(
        "git",
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["worktree", "list", "--porcelain"]),
    )?;

    Ok(parse(&porcelain)
        .into_iter()
        .map(|worktree| Worktree {
            path: canonical(worktree.path.as_str()),
            ..worktree
        })
        .collect())
}

/// Return the shared git directory (`--git-common-dir`) for the repository
/// containing `dir`.
pub fn common_dir(dir: &Utf8Path) -> Result<Utf8PathBuf, Error> {
    let out = stdout(
        "git",
        Command::new("git").arg("-C").arg(dir).args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ]),
    )?;

    Ok(canonical(out.trim()))
}

/// Return the root directory (`--show-toplevel`) of the worktree containing
/// `dir`.
pub fn toplevel(dir: &Utf8Path) -> Result<Utf8PathBuf, Error> {
    let out = stdout(
        "git",
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "--show-toplevel"]),
    )?;

    Ok(canonical(out.trim()))
}

/// Return the default branch name from `refs/remotes/origin/HEAD`, falling back
/// to `"main"`.
#[must_use]
pub fn default_branch(dir: &Utf8Path) -> String {
    stdout(
        "git",
        Command::new("git").arg("-C").arg(dir).args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ]),
    )
    .ok()
    .and_then(|name| Some(name.trim().strip_prefix("origin/")?.to_owned()))
    .filter(|name| !name.is_empty())
    .unwrap_or_else(|| "main".to_owned())
}

/// Fast-forward the local `branch` to its upstream and return `true` if its
/// commit changed.
///
/// If `branch` is checked out in a worktree, runs `git fetch` and `git merge
/// --ff-only` in that worktree so its working tree stays in sync (failing
/// without modifying files if the merge is not a fast-forward or conflicts with
/// local changes).
/// Otherwise, fetches `branch:branch` directly.
pub fn update_branch(dir: &Utf8Path, branch: &str) -> Result<bool, Error> {
    let _updating = UPDATING.lock().unwrap_or_else(PoisonError::into_inner);

    let checkout = worktrees(dir)?
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch));
    let reference = format!("refs/heads/{branch}");
    let before = rev_parse(dir, &reference)?;

    if let Some(worktree) = &checkout {
        let path = &worktree.path;
        git(path, &["fetch", "--quiet"])?;
        git(path, &["merge", "--ff-only", "--quiet", "@{upstream}"])?;
    } else {
        let remote = git(dir, &["config", &format!("branch.{branch}.remote")])
            .map_or_else(|_| "origin".to_owned(), |remote| remote.trim().to_owned());
        git(dir, &[
            "fetch",
            "--quiet",
            &remote,
            &format!("{branch}:{branch}"),
        ])?;
    }

    Ok(rev_parse(dir, &reference)? != before)
}

/// Validate `name` as a legal git branch name.
pub fn check_branch_name(dir: &Utf8Path, name: &str) -> Result<(), Error> {
    git(dir, &["check-ref-format", "--branch", name]).map(drop)
}

/// Add a worktree at `path`, detached at `commit`.
pub fn add_detached(dir: &Utf8Path, path: &Utf8Path, commit: &str) -> Result<(), Error> {
    git(dir, &["worktree", "add", "--detach", path.as_str(), commit]).map(drop)
}

/// Add a worktree at `path` on a new branch `branch`, starting at `from`.
pub fn add_branch(dir: &Utf8Path, path: &Utf8Path, branch: &str, from: &str) -> Result<(), Error> {
    git(dir, &["worktree", "add", "-b", branch, path.as_str(), from]).map(drop)
}

/// Resolve `revision` to a commit SHA.
fn rev_parse(dir: &Utf8Path, revision: &str) -> Result<String, Error> {
    git(dir, &["rev-parse", "--verify", "--quiet", revision]).map(|out| out.trim().to_owned())
}

/// Run `git` in `dir` without prompting, and return its standard output.
fn git(dir: &Utf8Path, args: &[&str]) -> Result<String, Error> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    stdout("git", noninteractive(&mut command, dir))
}

/// Force-remove the worktree at `path`, discarding any uncommitted changes.
pub fn remove_worktree(dir: &Utf8Path, path: &Utf8Path) -> Result<(), Error> {
    stdout(
        "git",
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["worktree", "remove", "--force"])
            .arg(path),
    )
    .map(drop)
}

/// Parse `git worktree list --porcelain` output.
///
/// Bare repositories and prunable worktrees (whose working directories no
/// longer exist) are omitted.
fn parse(porcelain: &str) -> Vec<Worktree> {
    porcelain
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut branch = None;

            for line in block.lines() {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                match key {
                    "worktree" => path = Some(value),
                    "branch" => branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value)),
                    "bare" | "prunable" => return None,
                    _ => {}
                }
            }

            Some(Worktree {
                path: path?.into(),
                branch: branch.map(str::to_owned),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
