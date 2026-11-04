//! The repository's worktrees, from `git worktree list`, and the git work the
//! dashboard does on them: keeping the default branch up to date, and adding
//! worktrees.
//!
//! Every command that may reach a remote runs with prompts disabled and without
//! a stdin: the dashboard owns the terminal, and a credential prompt would draw
//! over it and wait forever.

use std::{
    env,
    process::{Command, Stdio},
    sync::{Mutex, PoisonError},
};

use camino::{Utf8Path, Utf8PathBuf};

use super::{Error, canonical, stdout};
use crate::model::Worktree;

/// Held while the default branch is being updated, so a periodic update and one
/// started for a new worktree do not fetch into the same refs at once.
static UPDATING: Mutex<()> = Mutex::new(());

/// Make `command`, a `git` or `gh` command run in `dir`, fail rather than ask
/// for anything.
///
/// Git's own prompts are turned off, and SSH runs in batch mode on top of
/// whatever SSH command the user configured.
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

/// The git directory every worktree of the repository containing `dir` shares.
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

/// The root directory of the worktree containing `dir`.
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

/// The branch new worktrees start from: the one `origin`'s `HEAD` names, or
/// `main` when it names none.
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

/// Fast-forward the local `branch` to its upstream, and say whether it moved.
///
/// When a worktree has `branch` checked out, the update runs there, so the
/// files follow the branch; a merge that is not a fast-forward, or that would
/// overwrite uncommitted changes, fails without touching anything.
/// Otherwise the branch is fetched into directly.
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

/// Whether `name` is a valid name for a new branch.
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

/// The commit `revision` names.
fn rev_parse(dir: &Utf8Path, revision: &str) -> Result<String, Error> {
    git(dir, &["rev-parse", "--verify", "--quiet", revision]).map(|out| out.trim().to_owned())
}

/// Run `git` in `dir` without prompting, and return its standard output.
fn git(dir: &Utf8Path, args: &[&str]) -> Result<String, Error> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    stdout("git", noninteractive(&mut command, dir))
}

/// Remove the worktree at `path`, uncommitted changes and all.
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
/// A bare repository has no working directory and is left out, as is a worktree
/// `git` reports as prunable, whose directory is gone.
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
