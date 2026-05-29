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
/// the given path.
/// For a file, returns 0 or 1.
/// For a directory, returns the number of changed entries underneath it.
/// Returns 0 when outside a git repository.
///
/// Unlike `is_file_dirty`, this counts any non-empty porcelain line — staged,
/// unstaged, untracked, renamed, all of it.
/// Directory moves have a wider blast radius than file moves, so the dirty
/// check is correspondingly stricter.
///
/// Currently the only caller (`fs_move_file`) flows through a generic
/// `ProcessRunner` for testability, so the production wrapper is omitted — add
/// one if a non-generic caller appears.
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

/// Kind of filesystem entry, as observed via `symlink_metadata`.
///
/// Distinguishes the four shapes the fs tools care about.
/// The resolver layer already rejects dangling-symlink ancestors at
/// canonicalization time, so anything that reaches this helper has already
/// passed the workspace-escape check; the only remaining question is what kind
/// of entry sits at the final position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// Live *or* dangling symlink at the final position.
    /// The link entry exists; the resolver-level dangling check applies only to
    /// ancestors and to the `path`-style resolver, not to entries reached via
    /// [`resolve_workspace_entry`].
    Symlink,
    /// Block device, fifo, socket, etc. The fs tools refuse to operate on these
    /// because `fs::rename`/`fs::remove_file`/`File::open` behavior is too
    /// platform-dependent to be worth getting right.
    Other,
}

/// Stat an entry without following a final-position symlink.
///
/// Returns `Ok(None)` when the entry does not exist and `Ok(Some(kind))`
/// otherwise.
/// Use this in place of `Utf8Path::is_dir`/`is_file`/`exists` whenever the
/// caller has already routed through [`resolve_workspace_entry`] — those
/// `Path` methods follow symlinks and will lie about a dangling final-position
/// symlink (existence checks return `false` even though the entry is present
/// and `fs::rename` will silently replace it).
pub fn entry_kind(path: &Utf8Path) -> Result<Option<EntryKind>, io::Error> {
    match path.symlink_metadata() {
        Ok(m) => {
            let ft = m.file_type();
            let kind = if ft.is_symlink() {
                EntryKind::Symlink
            } else if ft.is_dir() {
                EntryKind::Dir
            } else if ft.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            Ok(Some(kind))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Maximum byte-length of any individual path component.
const MAX_COMPONENT_LEN: usize = 100;

/// Maximum number of `Normal` components in a user-supplied path.
const MAX_COMPONENT_COUNT: usize = 20;

/// A user-supplied path that has been resolved against the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Absolute path inside the workspace.
    ///
    /// Always anchored in the canonicalized workspace root.
    /// The shape of the final component depends on which resolver produced it:
    /// for [`resolve_workspace_path`] it is the canonical target (symlinks
    /// followed); for [`resolve_workspace_entry`] it is the directory entry as
    /// named by the user (symlinks left intact).
    pub absolute: Utf8PathBuf,

    /// Path relative to the canonical workspace root.
    pub relative: Utf8PathBuf,
}

/// Resolve a user-supplied path against the workspace root, following symlinks
/// all the way to the final component.
///
/// Use this when the caller wants to operate on the *content* of the target —
/// reading, modifying, or otherwise touching whatever the path ultimately
/// points to.
/// Read- and modify-style tools belong here (`fs_read_file`, `fs_modify_file`).
///
/// For write tools that should operate on the directory entry itself (create,
/// delete, rename), use [`resolve_workspace_entry`] instead — it does not
/// follow a final symlink and so cannot be tricked into writing or unlinking
/// through one.
///
/// For not-yet-existing targets, only the deepest existing ancestor is
/// canonicalized; the lexical remainder is appended as-is.
/// After cleaning, the remainder contains only `Normal` components and cannot
/// traverse upwards out of the canonicalized ancestor.
///
/// Dangling symlinks at any position in the path are rejected — there is no
/// canonical target to bound, so the workspace check cannot prove the path
/// stays inside.
pub fn resolve_workspace_path(root: &Utf8Path, path: &str) -> Result<ResolvedPath, String> {
    let ValidatedInput {
        cleaned,
        canonical_root,
    } = validate_workspace_input(root, path)?;

    let candidate = root.join(&cleaned);
    let (canonical_ancestor, suffix) = check_ancestor_in_root(&candidate, &canonical_root)?;

    let absolute = if suffix.as_str().is_empty() {
        canonical_ancestor
    } else {
        canonical_ancestor.join(&suffix)
    };

    let relative = absolute
        .strip_prefix(&canonical_root)
        .map(Utf8Path::to_owned)
        .map_err(|_| "Path escapes the workspace root.".to_owned())?;

    Ok(ResolvedPath { absolute, relative })
}

/// Resolve a user-supplied path as a directory entry, canonicalizing only the
/// *parent*.
///
/// The final component is preserved as-is: if it is an existing symlink, it is
/// left intact rather than followed.
/// This is the right primitive for tools that operate on the entry itself —
/// create, delete, rename — where following the link would silently retarget
/// the operation onto whatever the link points at.
///
/// All other validation rules from [`resolve_workspace_path`] still apply: the
/// parent must canonicalize to somewhere inside `canonical_root`, no
/// dangling-symlink ancestors, length limits, and so on.
pub fn resolve_workspace_entry(root: &Utf8Path, path: &str) -> Result<ResolvedPath, String> {
    let ValidatedInput {
        cleaned,
        canonical_root,
    } = validate_workspace_input(root, path)?;

    // `cleaned` is guaranteed non-empty by `validate_workspace_input`, so
    // `file_name()` cannot return None here.
    let final_name = cleaned
        .file_name()
        .ok_or_else(|| "Path has no final component.".to_owned())?
        .to_owned();
    let parent_rel = cleaned.parent().unwrap_or_else(|| Utf8Path::new(""));

    let parent_candidate = if parent_rel.as_str().is_empty() {
        root.to_owned()
    } else {
        root.join(parent_rel)
    };

    let (canonical_parent, parent_suffix) =
        check_ancestor_in_root(&parent_candidate, &canonical_root)?;

    // Reattach any not-yet-existing parent components, then the final name.
    // The final name is never canonicalized or probed for symlink-ness — its
    // shape is whatever the user supplied.
    let absolute = if parent_suffix.as_str().is_empty() {
        canonical_parent.join(&final_name)
    } else {
        canonical_parent.join(&parent_suffix).join(&final_name)
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
/// Performs the same input validation as [`resolve_workspace_path`], but
/// doesn't canonicalize the result.
/// Use this when output paths should match what the user supplied (read/search
/// tools).
pub fn clean_workspace_path(root: &Utf8Path, path: &str) -> Result<Utf8PathBuf, String> {
    let ValidatedInput {
        cleaned,
        canonical_root,
    } = validate_workspace_input(root, path)?;

    let candidate = root.join(&cleaned);
    check_ancestor_in_root(&candidate, &canonical_root)?;

    Ok(cleaned)
}

/// Output of [`validate_workspace_input`]: the cleaned form plus the
/// canonicalized workspace root.
/// Each public resolver decides how to canonicalize the rest.
struct ValidatedInput {
    /// Lexically-cleaned, non-canonical workspace-relative path.
    cleaned: Utf8PathBuf,
    /// Canonical workspace root (symlinks resolved).
    canonical_root: Utf8PathBuf,
}

/// Run the input-level validation shared by every workspace path resolver.
///
/// This only inspects the input and the workspace root — it does not touch the
/// rest of the filesystem.
/// Per-resolver canonicalization happens in [`check_ancestor_in_root`].
///
/// The input is lexically normalized first (via `clean-path`), so `foo/../bar`
/// is accepted and reduces to `bar`.
/// Paths whose normalized form still tries to climb above the workspace
/// (`../etc/passwd`, `foo/../../etc/passwd`) are rejected.
///
/// Rejects:
///
/// - Absolute paths (`/etc/passwd`, `C:\foo`).
/// - Rooted but not-fully-absolute paths (`\foo`, `\\server\share`).
///   These are caught by `has_root()` even when `is_absolute()` would return
///   false (Windows drive-relative paths).
/// - `Prefix(_)` components (Windows drive-relative inputs like `C:foo`) that
///   survive both rooted checks.
/// - Normalized paths that still contain a leading `..` (escape attempts).
/// - Components longer than 100 bytes, or paths with more than 20 components.
/// - Empty paths.
fn validate_workspace_input(root: &Utf8Path, path: &str) -> Result<ValidatedInput, String> {
    let raw = Utf8PathBuf::from(path);

    // `is_absolute()` and `has_root()` together cover most "rooted from
    // the filesystem" shapes. On Unix the two are equivalent. On Windows,
    // `has_root()` catches `\foo` and UNC paths that `is_absolute()`
    // (which also requires a drive prefix) misses. The `Prefix(_)` arm
    // below catches drive-relative inputs like `C:foo` that slip past
    // both.
    if raw.is_absolute() || raw.has_root() {
        return Err("Path must be relative.".to_owned());
    }

    // Lexical normalization only. `foo/../bar` collapses to `bar`,
    // `./foo` to `foo`, and so on. Any `..` that cannot be cancelled by an
    // earlier component remains as a leading `..` in the cleaned path,
    // which we reject below as an escape attempt. The actual security
    // boundary is the canonical-root `starts_with` check in
    // `check_ancestor_in_root`; this step just decides what to accept as
    // input.
    let cleaned: PathBuf = PathBuf::from(raw.as_str()).clean();
    let cleaned = Utf8PathBuf::from_path_buf(cleaned)
        .map_err(|p| format!("Path contains non-UTF-8 characters: {}", p.display()))?;

    let mut normal_count = 0usize;
    for component in cleaned.components() {
        match component {
            Utf8Component::ParentDir => {
                return Err("Path must not escape the workspace root.".to_owned());
            }
            Utf8Component::Prefix(_) => {
                // Drive-relative (`C:foo`) and UNC-ish inputs slip past
                // `is_absolute()` and `has_root()` on Windows. Treat any
                // surviving prefix as a non-relative input.
                return Err("Path must be relative.".to_owned());
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
            // After cleaning a relative path, RootDir is impossible (the
            // rooted-path checks above rejected it). CurDir survives only
            // as a bare `.` placeholder, which contributes nothing.
            Utf8Component::CurDir | Utf8Component::RootDir => {}
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

    Ok(ValidatedInput {
        cleaned,
        canonical_root,
    })
}

/// Canonicalize the deepest existing ancestor of `candidate` and verify it
/// stays inside `canonical_root`.
///
/// Returns `(canonical_ancestor, lexical_suffix)`.
/// When the full candidate exists, `lexical_suffix` is empty.
fn check_ancestor_in_root(
    candidate: &Utf8Path,
    canonical_root: &Utf8Path,
) -> Result<(Utf8PathBuf, Utf8PathBuf), String> {
    let (canonical_ancestor, suffix) = canonicalize_existing_ancestor(candidate)?;

    if !canonical_ancestor.starts_with(canonical_root) {
        return Err("Path escapes the workspace root.".to_owned());
    }

    Ok((canonical_ancestor, suffix))
}

/// Walk up `path` until an existing ancestor can be canonicalized.
///
/// Returns `(canonical_ancestor, lexical_suffix)`.
/// When the full path exists, `lexical_suffix` is empty.
///
/// A `NotFound` from `canonicalize_utf8()` is normally treated as "this
/// component doesn't exist yet, pop it and try the parent."
/// The exception is when the component itself exists as a *dangling* symlink:
/// `canonicalize` fails because the target is missing, but the entry is still
/// there and `open(O_CREAT)` would follow it and create the target outside the
/// workspace.
/// We probe with `symlink_metadata` to tell the two cases apart and reject the
/// symlink case.
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
                if current.symlink_metadata().is_ok() {
                    return Err(format!(
                        "Path '{current}' is a symlink with a missing or non-workspace target."
                    ));
                }
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
