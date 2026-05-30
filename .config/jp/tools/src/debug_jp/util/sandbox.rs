//! Isolated workspace for profiling runs.
//!
//! A [`Sandbox`] is a defense-in-depth wrapper that lets a profiling tool run
//! `jp` against a copy of the user's repo and conversation data without any
//! risk of mutating the live workspace.
//! If the assistant approves a destructive command by mistake, the damage is
//! contained to the sandbox and disappears when this struct is dropped.
//!
//! The sandbox combines two isolation mechanisms:
//!
//! 1. A `git worktree --detach` rooted under `<workspace>/tmp/`, with
//!    uncommitted tracked changes applied as a patch and untracked files copied
//!    across.
//!    Shares the parent's `target/` via the project's `.cargo/config.toml`, so
//!    `cargo build` is incremental.
//! 2. A scratch directory under `<workspace>/tmp/` that `JP_USER_DATA_DIR`
//!    points to.
//!    The current user data is optionally cloned in so the profile run has real
//!    conversations to operate on.
//!
//! Both paths are cleaned up by [`Sandbox`]'s `Drop` impl on a best-effort
//! basis.
//! Cleanup failures are logged to stderr but never panic.

use std::{
    fs, io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use reflink_copy::reflink_or_copy;

use crate::{
    Error,
    util::runner::{DuctProcessRunner, ProcessRunner},
};

/// Options controlling sandbox construction.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SandboxOpts {
    /// Clone the current user data directory into the sandbox so the profile
    /// run sees real conversations.
    /// When `false`, the sandbox starts with an empty user data directory.
    pub clone_user_data: bool,
}

impl Default for SandboxOpts {
    fn default() -> Self {
        Self {
            clone_user_data: true,
        }
    }
}

/// An isolated profiling environment.
/// See the module docs.
pub(crate) struct Sandbox {
    root: Utf8PathBuf,
    worktree: Utf8PathBuf,
    user_data: Utf8PathBuf,
}

impl Sandbox {
    /// Build a sandbox rooted at `workspace_root`.
    ///
    /// Creates a detached git worktree, applies the user's uncommitted state,
    /// and (optionally) clones their user data.
    /// Side effects are confined to the sandbox paths and are cleaned up on
    /// drop.
    pub(crate) fn create(
        workspace_root: &Utf8Path,
        opts: SandboxOpts,
        runner: &dyn ProcessRunner,
    ) -> Result<Self, Error> {
        let suffix = unique_suffix();
        let tmp = workspace_root.join("tmp");
        fs::create_dir_all(&tmp)?;

        // Reclaim sandboxes from earlier runs whose host process died before
        // its `Drop` could clean up. Cheap, best-effort, and keyed on the PID
        // embedded in each sandbox name.
        sweep_stale_sandboxes(runner, workspace_root, &tmp);

        let worktree = tmp.join(format!("jp-sandbox-{suffix}"));
        let user_data = tmp.join(format!("jp-sandbox-data-{suffix}"));

        // git worktree add --detach: HEAD checkout, no branch to clean up.
        run_git(runner, workspace_root, &[
            "worktree",
            "add",
            "--detach",
            worktree.as_str(),
        ])?;

        // Best-effort cleanup if any of the following steps fails after the
        // worktree exists.
        let mut sandbox = Self {
            root: workspace_root.to_owned(),
            worktree: worktree.clone(),
            user_data: user_data.clone(),
        };

        apply_uncommitted(runner, workspace_root, &worktree)?;
        copy_untracked(runner, workspace_root, &worktree)?;

        // `clone_user_data_into` needs `user_data` to NOT exist so `cp -R`
        // creates it as a clone of the source. When skipping the clone, we
        // create an empty dir directly so `JP_USER_DATA_DIR` resolves to a
        // real path with the expected (empty) shape.
        if opts.clone_user_data {
            clone_user_data_into(&user_data)?;
        } else {
            fs::create_dir_all(&user_data)?;
        }

        // Move ownership only after every fallible step succeeds; until now
        // the local `sandbox` value holds the cleanup responsibility.
        sandbox.worktree = worktree;
        sandbox.user_data = user_data;
        Ok(sandbox)
    }

    /// Directory `cargo build` and the launched `jp` should run from.
    pub(crate) fn working_dir(&self) -> &Utf8Path {
        &self.worktree
    }

    /// Environment overrides to apply when launching `jp` inside the sandbox.
    pub(crate) fn env(&self) -> Vec<(String, String)> {
        vec![("JP_USER_DATA_DIR".to_owned(), self.user_data.to_string())]
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let runner = DuctProcessRunner;
        remove_worktree(&runner, &self.root, &self.worktree);

        if let Err(error) = fs::remove_dir_all(&self.user_data)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "sandbox: failed to remove user data at {}: {error}",
                self.user_data
            );
        }

        // Clear any `git worktree` registration the rm fallback left behind.
        drop(run_git(&runner, &self.root, &["worktree", "prune"]));
    }
}

/// Unix-epoch seconds + process ID, unique enough across concurrent runs.
fn unique_suffix() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("{secs}-{}", std::process::id())
}

/// Remove a sandbox worktree via git so the `<repo>/.git/worktrees/<name>`
/// registration is cleaned up too, falling back to a plain recursive delete
/// when git is unhappy.
///
/// `--force` because uncommitted changes inside the sandbox are exactly what we
/// want to throw away.
/// Best-effort: failures are logged, never fatal.
fn remove_worktree(runner: &dyn ProcessRunner, workspace_root: &Utf8Path, worktree: &Utf8Path) {
    if !worktree.exists() {
        return;
    }
    if let Err(error) = run_git(runner, workspace_root, &[
        "worktree",
        "remove",
        "--force",
        worktree.as_str(),
    ]) {
        eprintln!("sandbox: failed to remove worktree at {worktree}: {error}");
        drop(fs::remove_dir_all(worktree));
    }
}

/// Remove sandboxes left behind by previous runs whose owning process is gone.
///
/// Each sandbox directory embeds the creating PID (see [`unique_suffix`]), so a
/// dead PID means that run's `Drop` never executed (its host was killed).
/// Live PIDs — concurrent runs and our own — are skipped.
/// Names we can't parse are left alone rather than guessed at.
/// Best-effort; never aborts creation.
fn sweep_stale_sandboxes(runner: &dyn ProcessRunner, workspace_root: &Utf8Path, tmp: &Utf8Path) {
    let Ok(entries) = fs::read_dir(tmp.as_std_path()) else {
        return;
    };

    let mut removed_worktree = false;
    for entry in entries.flatten() {
        let raw_name = entry.file_name();
        let Some(name) = raw_name.to_str() else {
            continue;
        };

        // Data dirs share the `jp-sandbox-` prefix, so check the longer one
        // first to classify the entry correctly.
        let is_data = name.starts_with("jp-sandbox-data-");
        let suffix = if is_data {
            name.strip_prefix("jp-sandbox-data-")
        } else {
            name.strip_prefix("jp-sandbox-")
        };
        let Some(suffix) = suffix else {
            continue;
        };

        // Suffix is `<secs>-<pid>`; the PID is the trailing component.
        let Some(pid) = suffix
            .rsplit('-')
            .next()
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if pid_is_alive(pid) {
            continue;
        }

        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
            continue;
        };
        if is_data {
            drop(fs::remove_dir_all(&path));
        } else {
            remove_worktree(runner, workspace_root, &path);
            removed_worktree = true;
        }
    }

    if removed_worktree {
        drop(run_git(runner, workspace_root, &["worktree", "prune"]));
    }
}

/// Whether a process with `pid` is currently alive.
///
/// On non-unix targets, conservatively returns `true` so the sweep never
/// deletes a sandbox it can't prove is abandoned.
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    // `kill(pid, 0)` does the kernel's permission/existence checks without
    // sending a signal: 0 => alive; EPERM => alive but not ours; ESRCH => no
    // such process.
    if unsafe { libc::kill(pid.cast_signed(), 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    true
}

/// Apply tracked uncommitted changes (staged + unstaged) from `source` into
/// `target`.
/// No-op when the working tree is clean.
fn apply_uncommitted(
    runner: &dyn ProcessRunner,
    source: &Utf8Path,
    target: &Utf8Path,
) -> Result<(), Error> {
    let patch = run_git_capture(runner, source, &["diff", "HEAD", "--binary"])?;
    if patch.trim().is_empty() {
        return Ok(());
    }

    let output = runner
        .run_with_env_and_stdin(
            "git",
            &["apply", "--index", "--whitespace=nowarn"],
            target,
            &[],
            Some(patch.as_str()),
        )
        .map_err(|e| format!("Failed to spawn `git apply`: {e}"))?;
    if !output.success() {
        return Err(format!("`git apply` failed in sandbox: {}", output.stderr).into());
    }
    Ok(())
}

/// Copy untracked-but-not-ignored files from `source` into `target`.
fn copy_untracked(
    runner: &dyn ProcessRunner,
    source: &Utf8Path,
    target: &Utf8Path,
) -> Result<(), Error> {
    let list = run_git_capture(runner, source, &[
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ])?;

    for entry in list.split('\0').filter(|s| !s.is_empty()) {
        let src = source.join(entry);
        let dst = target.join(entry);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        // Skip symlinks and directories — `ls-files --others` only emits
        // files, but defensively check to avoid surprises.
        let meta = fs::symlink_metadata(&src)?;
        if meta.file_type().is_file() {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Clone the current user data directory so that `target` ends up as a copy of
/// it, preferring copy-on-write where the filesystem supports it.
///
/// When the source doesn't exist (fresh workspace, no conversations), creates
/// an empty `target` instead so `JP_USER_DATA_DIR` still resolves cleanly.
///
/// On macOS, [`reflink_or_copy`] accepts a directory and uses `clonefile(2)` to
/// clone the entire hierarchy in a single syscall — effectively free
/// regardless of size.
/// On other platforms, and on macOS when source/target span volumes (`EXDEV`),
/// the fast path errors and we fall back to a walker that reflinks each file
/// individually.
fn clone_user_data_into(target: &Utf8Path) -> Result<(), Error> {
    let source = jp_workspace::user_data_dir()
        .map_err(|e| format!("Failed to resolve current user data dir: {e}"))?;

    if !source.exists() {
        fs::create_dir_all(target)?;
        return Ok(());
    }

    // macOS fast path: one clonefile(2) for the whole tree.
    #[cfg(target_os = "macos")]
    if reflink_or_copy(&source, target).is_ok() {
        return Ok(());
    }

    // Fallback walker. If the macOS fast path left a partial directory
    // behind (e.g. cross-volume failure mid-clone), clear it first so the
    // walker starts from a clean slate.
    drop(fs::remove_dir_all(target));
    copy_dir_recursive(source.as_std_path(), target.as_std_path())
        .map_err(|e| format!("Failed to clone user data from {source} to {target}: {e}").into())
}

/// Recursively copy `source` into `target`, reflinking each file where the
/// filesystem supports copy-on-write (Linux btrfs/xfs `FICLONE`, Windows
/// `ReFS`, macOS APFS) and falling back to a regular copy otherwise.
///
/// Symlinks are dereferenced.
/// Non-regular, non-directory entries (sockets, fifos, etc.) are skipped —
/// JP's data dir shouldn't contain them, but defensive against surprises.
fn copy_dir_recursive(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let src = entry.path();
        let dst = target.join(entry.file_name());

        // `fs::metadata` follows symlinks, so a symlink-to-dir is treated
        // as a dir, and a symlink-to-file as a file. A dangling symlink
        // raises `NotFound`, which we skip.
        let meta = match fs::metadata(&src) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };

        if meta.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else if meta.is_file() {
            reflink_or_copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Run `git` with the given args from `dir`, expecting success.
fn run_git(runner: &dyn ProcessRunner, dir: &Utf8Path, args: &[&str]) -> Result<(), Error> {
    let output = runner
        .run("git", args, dir)
        .map_err(|e| format!("Failed to spawn `git {}`: {e}", args.join(" ")))?;
    if !output.success() {
        return Err(format!("`git {}` failed: {}", args.join(" "), output.stderr).into());
    }
    Ok(())
}

/// Run `git` from `dir` and return its stdout.
fn run_git_capture(
    runner: &dyn ProcessRunner,
    dir: &Utf8Path,
    args: &[&str],
) -> Result<String, Error> {
    let output = runner
        .run("git", args, dir)
        .map_err(|e| format!("Failed to spawn `git {}`: {e}", args.join(" ")))?;
    if !output.success() {
        return Err(format!("`git {}` failed: {}", args.join(" "), output.stderr).into());
    }
    Ok(output.stdout)
}
