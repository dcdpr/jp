use std::{io, path::PathBuf};

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use clean_path::Clean as _;

use crate::{
    Error,
    util::runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub fn is_file_dirty(root: &Utf8Path, file: &Utf8Path) -> Result<bool, Error> {
    is_file_dirty_impl(root, file, &DuctProcessRunner)
}

pub(super) fn is_file_dirty_impl<R: ProcessRunner>(
    root: &Utf8Path,
    file: &Utf8Path,
    runner: &R,
) -> Result<bool, Error> {
    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run("git", &["status", "--porcelain", "--", file.as_str()], root)?;

    if stderr.contains("fatal: not a git repository") {
        return Ok(false);
    }

    if !status.is_success() {
        return Err(format!("Git command failed ({status}): {stderr}").into());
    }

    // The second column is the non-staged status indicator.
    Ok(stdout.chars().nth(1) == Some('M'))
}

/// Count the number of dirty entries reported by `git status --porcelain` for
/// the given path. For a file, returns 0 or 1. For a directory, returns the
/// number of changed entries underneath it. Returns 0 when outside a git
/// repository.
///
/// Unlike `is_file_dirty`, this counts any non-empty porcelain line — staged,
/// unstaged, untracked, renamed, all of it. Directory moves have a wider
/// blast radius than file moves, so the dirty check is correspondingly
/// stricter.
///
/// Currently the only caller (`fs_move_file`) flows through a generic
/// `ProcessRunner` for testability, so the production wrapper is omitted —
/// add one if a non-generic caller appears.
pub(super) fn count_dirty_paths_impl<R: ProcessRunner>(
    root: &Utf8Path,
    path: &Utf8Path,
    runner: &R,
) -> Result<usize, Error> {
    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run("git", &["status", "--porcelain", "--", path.as_str()], root)?;

    if stderr.contains("fatal: not a git repository") {
        return Ok(0);
    }

    if !status.is_success() {
        return Err(format!("Git command failed ({status}): {stderr}").into());
    }

    Ok(stdout.lines().filter(|l| !l.is_empty()).count())
}

/// Maximum byte-length of any individual path component.
const MAX_COMPONENT_LEN: usize = 100;

/// Maximum number of `Normal` components in a user-supplied path.
const MAX_COMPONENT_COUNT: usize = 20;

/// A user-supplied path that has been resolved against the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Absolute path with the deepest existing ancestor canonicalized.
    /// Guaranteed to lie within the canonical workspace root.
    pub absolute: Utf8PathBuf,

    /// Path relative to the canonical workspace root.
    pub relative: Utf8PathBuf,
}

/// Resolve a user-supplied path against the workspace root.
///
/// See [`check_workspace_path`] for the validation contract. The returned
/// `absolute` path is canonicalized through symlinks on existing ancestors,
/// making it safe for I/O. Use this when the canonical form is what you want
/// to operate on (writes, git interactions).
///
/// For targets that do not yet exist (e.g. when creating a new file), only
/// the deepest existing ancestor is canonicalized; the lexical remainder is
/// appended as-is. This is safe because, after cleaning, the remainder
/// contains only `Normal` components and cannot traverse upwards out of the
/// canonicalized ancestor.
pub fn resolve_workspace_path(root: &Utf8Path, path: &str) -> Result<ResolvedPath, String> {
    let CheckedPath {
        canonical_root,
        canonical_ancestor,
        suffix,
        ..
    } = check_workspace_path(root, path)?;

    let absolute = if suffix.as_str().is_empty() {
        canonical_ancestor.clone()
    } else {
        canonical_ancestor.join(&suffix)
    };

    let relative = absolute
        .strip_prefix(&canonical_root)
        .map(Utf8Path::to_owned)
        .map_err(|_| "Path escapes the workspace root.".to_owned())?;

    Ok(ResolvedPath { absolute, relative })
}

/// Clean a user-supplied path against the workspace root.
///
/// Returns the lexically-normalized, workspace-relative form, preserving the
/// caller's input shape — symlinks in existing ancestors are *checked* for
/// escape but not *followed* in the returned path.
///
/// Performs the same security checks as [`resolve_workspace_path`] (see
/// [`check_workspace_path`] for the contract), but doesn't canonicalize the
/// result. Use this when output paths should match what the user supplied
/// (read/search tools); use `resolve_workspace_path` when you need the
/// canonical form for I/O (write tools).
pub fn clean_workspace_path(root: &Utf8Path, path: &str) -> Result<Utf8PathBuf, String> {
    Ok(check_workspace_path(root, path)?.cleaned)
}

/// Output of `check_workspace_path`: the cleaned input form plus the canonical
/// pieces needed by `resolve_workspace_path`. Internal — callers go through
/// either `resolve_workspace_path` or `clean_workspace_path`.
struct CheckedPath {
    /// Lexically-cleaned, non-canonical workspace-relative path.
    cleaned: Utf8PathBuf,
    /// Canonical workspace root (symlinks resolved).
    canonical_root: Utf8PathBuf,
    /// Canonical absolute path of the deepest existing ancestor of
    /// `root.join(cleaned)`.
    canonical_ancestor: Utf8PathBuf,
    /// Lexical remainder appended to `canonical_ancestor` to reach the
    /// (possibly not-yet-existing) target. Empty when the full path exists.
    suffix: Utf8PathBuf,
}

/// Run the shared validation pipeline for workspace-bound paths.
///
/// The input is lexically normalized first (via `clean-path`), so
/// `foo/../bar` is accepted and reduces to `bar`. Paths whose normalized
/// form still tries to climb above the workspace (`../etc/passwd`,
/// `foo/../../etc/passwd`) are rejected.
///
/// Rejects:
///
/// - Absolute paths (`/etc/passwd`, `C:\foo`).
/// - Rooted but not-fully-absolute paths (`\foo`, `\\server\share`). These
///   are caught by `has_root()` even when `is_absolute()` would return
///   false (Windows drive-relative paths).
/// - Normalized paths that still contain a leading `..` (escape attempts).
/// - Paths whose deepest existing ancestor, after symlink resolution, lies
///   outside the canonicalized workspace root (defeats `linkdir/foo` where
///   `linkdir` is a symlink to `/etc`).
/// - Components longer than 100 bytes, or paths with more than 20 components.
/// - Empty paths.
fn check_workspace_path(root: &Utf8Path, path: &str) -> Result<CheckedPath, String> {
    let raw = Utf8PathBuf::from(path);

    // `is_absolute()` and `has_root()` together cover every "rooted from
    // the filesystem" shape across platforms. On Unix the two are
    // equivalent. On Windows, `has_root()` catches `\foo` and UNC paths
    // that `is_absolute()` (which also requires a drive prefix) misses.
    if raw.is_absolute() || raw.has_root() {
        return Err("Path must be relative.".to_owned());
    }

    // Lexical normalization only. `foo/../bar` collapses to `bar`,
    // `./foo` to `foo`, and so on. Any `..` that cannot be cancelled by an
    // earlier component remains as a leading `..` in the cleaned path,
    // which we reject below as an escape attempt. The actual security
    // boundary is the canonical-root `starts_with` check further down;
    // this step just decides what to accept as input.
    let cleaned: PathBuf = PathBuf::from(raw.as_str()).clean();
    let cleaned = Utf8PathBuf::from_path_buf(cleaned)
        .map_err(|p| format!("Path contains non-UTF-8 characters: {}", p.display()))?;

    let mut normal_count = 0usize;
    for component in cleaned.components() {
        match component {
            Utf8Component::ParentDir => {
                return Err("Path must not escape the workspace root.".to_owned());
            }
            Utf8Component::Normal(name) => {
                if name.len() > MAX_COMPONENT_LEN {
                    return Err(format!(
                        "Individual path components must be less than {MAX_COMPONENT_LEN} \
                         characters long."
                    ));
                }
                normal_count += 1;
            }
            // After cleaning a relative path, only Normal, CurDir, and
            // ParentDir components can appear. RootDir/Prefix are impossible
            // here because the rooted-path checks above already rejected
            // them.
            Utf8Component::CurDir | Utf8Component::RootDir | Utf8Component::Prefix(_) => {}
        }
    }

    if normal_count == 0 {
        return Err("Path must not be empty.".to_owned());
    }
    if normal_count > MAX_COMPONENT_COUNT {
        return Err(format!(
            "Path must be less than {MAX_COMPONENT_COUNT} components long."
        ));
    }

    let canonical_root = root
        .canonicalize_utf8()
        .map_err(|e| format!("Failed to canonicalize workspace root '{root}': {e}"))?;

    let candidate = root.join(&cleaned);
    let (canonical_ancestor, suffix) = canonicalize_existing_ancestor(&candidate)?;

    if !canonical_ancestor.starts_with(&canonical_root) {
        return Err("Path escapes the workspace root.".to_owned());
    }

    Ok(CheckedPath {
        cleaned,
        canonical_root,
        canonical_ancestor,
        suffix,
    })
}

/// Walk up `path` until an existing ancestor can be canonicalized.
///
/// Returns `(canonical_ancestor, lexical_suffix)`. When the full path exists,
/// `lexical_suffix` is empty.
fn canonicalize_existing_ancestor(path: &Utf8Path) -> Result<(Utf8PathBuf, Utf8PathBuf), String> {
    let mut current = path.to_owned();
    let mut suffix: Vec<String> = Vec::new();

    loop {
        match current.canonicalize_utf8() {
            Ok(canonical) => {
                let mut remainder = Utf8PathBuf::new();
                for name in suffix.iter().rev() {
                    remainder.push(name);
                }
                return Ok((canonical, remainder));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let Some(name) = current.file_name() else {
                    return Err(format!("Cannot find existing ancestor for path '{path}'."));
                };
                let name = name.to_owned();
                let Some(parent) = current.parent().map(Utf8Path::to_owned) else {
                    return Err(format!("Path '{path}' has no existing ancestor."));
                };
                suffix.push(name);
                current = parent;
            }
            Err(e) => {
                return Err(format!(
                    "Failed to canonicalize path component '{current}': {e}"
                ));
            }
        }
    }
}

#[cfg(test)]
#[path = "utils_tests.rs"]
mod tests;
