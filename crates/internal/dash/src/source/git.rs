//! Git worktree discovery and branch operations.
//!
//! Every command that may contact a remote runs with stdin closed and
//! interactive prompts disabled so a credential prompt cannot draw over the
//! dashboard TUI or block a background thread indefinitely.
//!
//! [`rebase_stack`] rebases a stack of pull request branches the way GitHub's
//! "Rebase stack" button does, which has no public API.

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

/// Read the git config value `key` for the repository containing `dir`.
///
/// `None` when the key is unset or empty, or `git` fails.
#[must_use]
pub fn config(dir: &Utf8Path, key: &str) -> Option<String> {
    git(dir, &["config", "--get", key])
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
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

/// The remote of the repository containing `dir` that points at GitHub
/// repository `repo` (`owner/name`), if any.
#[must_use]
pub fn remote_for(dir: &Utf8Path, repo: &str) -> Option<String> {
    let remotes = git(dir, &["remote"]).ok()?;

    remotes
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty())
        .find(|remote| {
            git(dir, &["remote", "get-url", remote]).is_ok_and(|url| points_at(url.trim(), repo))
        })
        .map(str::to_owned)
}

/// Whether remote `url` (HTTPS or SSH) names GitHub repository `repo`.
fn points_at(url: &str, repo: &str) -> bool {
    let url = url.to_lowercase();
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);

    url.strip_suffix(&repo.to_lowercase())
        .is_some_and(|rest| rest.ends_with('/') || rest.ends_with(':'))
}

/// How [`rebase_stack`] ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Rebased {
    /// The stack was rebased and pushed.
    Pushed,

    /// A branch moved since GitHub reported it; nothing was pushed.
    Moved,

    /// A commit didn't apply; nothing was pushed.
    Conflicts,

    /// Nothing was pushed, for this reason.
    Refused(String),
}

/// Rebase a stack of branches onto `base` and force-push them to `remote`.
///
/// `branches` are the stack's branches with the head commit each is expected
/// at, bottom first.
/// `below` is the head commit of the pull request below the bottom one, which
/// may have merged already: when the bottom branch builds on it, only the
/// commits above it move.
///
/// The rebase runs in a temporary worktree with git hooks and commit signing
/// off.
/// Every branch is pushed in one atomic push, each with a lease on its expected
/// head commit, so a branch someone pushed to in the meantime fails the whole
/// push.
///
/// Fails only when fetching fails, which is worth retrying.
pub fn rebase_stack(
    dir: &Utf8Path,
    remote: &str,
    base: &str,
    branches: &[(String, String)],
    below: Option<&str>,
) -> Result<Rebased, Error> {
    let tracking = |branch: &str| format!("refs/remotes/{remote}/{branch}");

    let mut fetch = vec![
        "fetch".to_owned(),
        "--quiet".to_owned(),
        "--no-tags".to_owned(),
        remote.to_owned(),
        format!("+refs/heads/{base}:{}", tracking(base)),
    ];
    fetch.extend(
        branches
            .iter()
            .map(|(branch, _)| format!("+refs/heads/{branch}:{}", tracking(branch))),
    );
    git(dir, &fetch.iter().map(String::as_str).collect::<Vec<_>>())?;

    let tip = rev_parse(dir, &tracking(base))?;
    let moved = branches
        .iter()
        .any(|(branch, sha)| rev_parse(dir, &tracking(branch)).ok().as_ref() != Some(sha));
    if moved {
        return Ok(Rebased::Moved);
    }

    let scratch = match camino_tempfile::Builder::new()
        .prefix("dash-rebase-")
        .tempdir()
    {
        Ok(scratch) => scratch,
        Err(error) => {
            return Ok(Rebased::Refused(format!(
                "creating a worktree failed: {error}"
            )));
        }
    };
    let worktree = scratch.path().join("stack");
    if let Err(error) = add_detached(dir, &worktree, &tip) {
        return Ok(Rebased::Refused(format!(
            "creating a worktree failed: {error}"
        )));
    }

    let replayed = replay(dir, &worktree, &tip, branches, below);
    // A worktree that won't go away only leaves a record `git worktree prune`
    // clears; its directory goes with `scratch`.
    let _kept = remove_worktree(dir, &worktree);

    let push = match replayed {
        Ok(push) => push,
        Err(outcome) => return Ok(outcome),
    };
    let mut args = vec!["push", "--quiet", "--atomic", "--no-verify", remote];
    args.extend(push.iter().map(String::as_str));

    Ok(match git(dir, &args) {
        Ok(_) => Rebased::Pushed,
        Err(error) => Rebased::Refused(format!("pushing the rebased stack failed: {error}")),
    })
}

/// Rebase each of `branches` in `worktree` onto the one below it, the bottom
/// one onto `tip`, and return the arguments that push the results.
fn replay(
    dir: &Utf8Path,
    worktree: &Utf8Path,
    tip: &str,
    branches: &[(String, String)],
    below: Option<&str>,
) -> Result<Vec<String>, Rebased> {
    let Some((_, bottom)) = branches.first() else {
        return Ok(vec![]);
    };

    let mut old = below
        .filter(|below| git(dir, &["merge-base", "--is-ancestor", below, bottom]).is_ok())
        .map(str::to_owned)
        .or_else(|| {
            git(dir, &["merge-base", tip, bottom])
                .ok()
                .map(|out| out.trim().to_owned())
        })
        .ok_or_else(|| Rebased::Refused(format!("{bottom} shares no history with its base")))?;
    let mut new = tip.to_owned();
    let mut push = vec![];

    for (branch, sha) in branches {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(worktree)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgSign=false",
                "rebase",
                "--quiet",
                // Moving other local branches would touch the user's worktrees.
                "--no-update-refs",
                "--no-autosquash",
                "--onto",
                &new,
                &old,
                sha,
            ])
            .env("GIT_EDITOR", "true");
        if stdout("git", noninteractive(&mut command, worktree)).is_err() {
            let _aborted = git(worktree, &["rebase", "--abort"]);
            return Err(Rebased::Conflicts);
        }

        old.clone_from(sha);
        new = rev_parse(worktree, "HEAD")
            .map_err(|error| Rebased::Refused(format!("rebasing the stack failed: {error}")))?;
        push.push(format!("--force-with-lease=refs/heads/{branch}:{sha}"));
        push.push(format!("{new}:refs/heads/{branch}"));
    }

    Ok(push)
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
