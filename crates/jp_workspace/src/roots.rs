//! Roots registry: the checkouts on disk that belong to a workspace ID.
//!
//! One workspace ID can resolve to several checkouts (for example git worktrees
//! of the same repository).
//! Each checkout registers itself in the workspace's user-workspace directory
//! as its own file:
//!
//! ```text
//! <user-workspace dir>/roots/<root-key>.json
//! ```
//!
//! `<root-key>` is a stable hash of the checkout's canonical path, so each
//! checkout owns exactly one file and concurrent runs never contend on shared
//! state.
//! Liveness is derived, never stored: a recorded root is live when it still
//! holds a workspace whose ID matches the directory's.
//! Dead entries are pruned opportunistically whenever the registry is read.
//!
//! [`resolve_live_roots`] expands a workspace ID to its live checkouts;
//! [`upsert_root`] registers the checkout a command runs against.
//!
//! See: `docs/rfd/087-session-scoped-active-workspace.md`

use std::{cmp::Reverse, collections::HashSet, fs, io};

use camino::{Utf8DirEntry, Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Duration, Utc};
use jp_storage::{
    matching_user_workspace_dirs,
    value::{read_json, write_json},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{trace, warn};

use crate::{Id, Workspace, error::Result};

/// Directory inside a user-workspace directory holding the per-root registry
/// files.
const ROOTS_DIR: &str = "roots";

/// Legacy single-checkout back-pointer, superseded by the roots registry.
const LEGACY_STORAGE_LINK: &str = "storage";

/// How recent a recorded `last_used` can be for [`upsert_root`] to skip
/// rewriting the entry, in minutes.
///
/// Recency only feeds display ordering and `latest` targeting, where sub-minute
/// precision carries no meaning, so a fresh entry is left untouched rather than
/// rewritten on every run.
/// Skipping the rewrite keeps repeated `jp` runs from churning the user-global
/// data directory: an external file watcher restarting a long-running `jp`
/// process on data-directory changes would otherwise re-trigger itself on every
/// restart, forever.
const REFRESH_GRANULARITY_MINUTES: i64 = 5;

/// A registered checkout of a workspace.
///
/// The on-disk shape of a single `roots/<root-key>.json` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootEntry {
    /// Canonical path of the checkout root.
    pub path: Utf8PathBuf,

    /// When a JP command last ran against this checkout.
    pub last_used: DateTime<Utc>,
}

/// Record `root` as a checkout in the user-workspace directory at
/// `user_workspace_dir`.
///
/// The path is canonicalized, then the checkout's own registry file is upserted
/// with a fresh `last_used` timestamp.
/// An existing entry still within the refresh granularity (see
/// `REFRESH_GRANULARITY_MINUTES`) is left untouched, so repeated runs against
/// the same checkout don't rewrite the file each time.
/// No other checkout's file is read or written, so concurrent runs from
/// different checkouts never contend.
pub fn upsert_root(user_workspace_dir: &Utf8Path, root: &Utf8Path) -> Result<()> {
    let path = root.canonicalize_utf8()?;
    let file = user_workspace_dir
        .join(ROOTS_DIR)
        .join(format!("{}.json", root_key(&path)));
    let now = Utc::now();

    // Leave a fresh, matching entry untouched. A `last_used` in the future
    // (clock rollback, tampered file) is not fresh: it is rewritten to `now`
    // so a bogus timestamp cannot pin recency ordering indefinitely.
    if let Ok(existing) = read_json::<RootEntry>(&file) {
        let age = now - existing.last_used;
        if existing.path == path
            && age >= Duration::zero()
            && age < Duration::minutes(REFRESH_GRANULARITY_MINUTES)
        {
            trace!(root = %path, file = %file, "Workspace checkout already fresh in roots registry.");
            return Ok(());
        }
    }

    let entry = RootEntry {
        path,
        last_used: now,
    };

    write_json(&file, &entry)?;
    trace!(root = %entry.path, file = %file, "Recorded workspace checkout in roots registry.");
    Ok(())
}

/// Expand workspace `id` to its live checkout roots.
///
/// Scans `workspaces_dir` (the per-user `workspace/` data directory) for the
/// workspace's user-workspace directories, folds any legacy `storage` symlink
/// into the registry, prunes dead entries, and returns the live roots most
/// recently used first.
///
/// A root is live when it still holds a workspace whose loaded ID equals `id`;
/// a deleted checkout, or one re-initialized as a different workspace, is
/// pruned.
#[must_use]
pub fn resolve_live_roots(workspaces_dir: &Utf8Path, id: &Id, storage_dir: &str) -> Vec<RootEntry> {
    let mut roots = vec![];
    for dir in matching_user_workspace_dirs(workspaces_dir, id) {
        migrate_legacy_symlink(&dir, id, storage_dir);
        roots.extend(live_roots(&dir, id, storage_dir));
    }

    // Most recently used first. When the same checkout is recorded in more
    // than one user-workspace directory (a legacy per-worktree state), keep
    // the freshest entry.
    roots.sort_by_key(|entry| Reverse(entry.last_used));
    let mut seen = HashSet::new();
    roots.retain(|entry| seen.insert(entry.path.clone()));
    roots
}

/// Fold a legacy `storage` symlink into the roots registry.
///
/// Older versions kept one `storage` symlink per user-workspace directory,
/// pointing at the last-used checkout's storage directory.
/// A link whose target still resolves to a workspace with a matching ID seeds a
/// registry entry; a dead or mismatched target seeds nothing.
/// The link is removed either way, so the migration runs at most once per
/// directory.
///
/// Best-effort: failures are logged, never fatal.
/// A checkout that fails to seed re-registers itself on the next run from
/// inside it.
pub fn migrate_legacy_symlink(user_workspace_dir: &Utf8Path, id: &Id, storage_dir: &str) {
    let link = user_workspace_dir.join(LEGACY_STORAGE_LINK);
    if !link.is_symlink() {
        return;
    }

    // Canonicalizing follows the link, so a deleted target fails here and
    // falls through to removal without seeding.
    let root = link
        .canonicalize_utf8()
        .ok()
        .and_then(|target| Workspace::find_root(target, storage_dir))
        .filter(|root| is_live(root, id, storage_dir));

    if let Some(root) = root {
        if let Err(error) = upsert_root(user_workspace_dir, &root) {
            // Keep the link so a later run can retry the seed.
            warn!(%error, %root, "Failed to seed roots registry from legacy storage symlink.");
            return;
        }
        trace!(%root, "Seeded roots registry from legacy storage symlink.");
    }

    if let Err(error) = remove_symlink(&link) {
        warn!(%error, link = %link, "Failed to remove legacy storage symlink.");
    }
}

/// Read a user-workspace directory's roots registry, pruning entries whose
/// checkout is gone.
///
/// Returns the live entries; a dead or unreadable entry's file is deleted
/// (best-effort) so the registry self-cleans as it is read.
fn live_roots(user_workspace_dir: &Utf8Path, id: &Id, storage_dir: &str) -> Vec<RootEntry> {
    let mut live = vec![];

    for file in registry_files(&user_workspace_dir.join(ROOTS_DIR)) {
        match read_json::<RootEntry>(&file) {
            Ok(entry) if is_live(&entry.path, id, storage_dir) => live.push(entry),
            Ok(entry) => {
                trace!(root = %entry.path, file = %file, "Pruning dead workspace root entry.");
                prune(&file);
            }
            Err(error) => {
                warn!(%error, file = %file, "Pruning unreadable workspace root entry.");
                prune(&file);
            }
        }
    }

    live
}

/// Whether `root` still holds a workspace whose loaded ID equals `id`.
///
/// The check is deliberately direct — `<root>/<storage_dir>` must itself be
/// the workspace's storage directory — rather than a walk-up discovery, so a
/// deleted checkout nested inside another checkout of the same workspace does
/// not masquerade as live.
#[must_use]
pub fn is_live(root: &Utf8Path, id: &Id, storage_dir: &str) -> bool {
    let storage = root.join(storage_dir);
    storage.is_dir() && matches!(Id::load(&storage), Some(Ok(loaded)) if loaded == *id)
}

/// A workspace known to the per-user `workspace/` data directory.
///
/// The listing unit behind the `jp w` picker, `jp w ls`, and fuzzy targeting:
/// one entry per workspace ID, expanded to its live checkouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownWorkspace {
    /// The workspace ID.
    pub id: Id,

    /// The cosmetic display name, from the user-workspace directory's
    /// `<slug>-<id>` name.
    ///
    /// May be absent (a bare `<id>` name), is never renamed, and is not unique
    /// across workspaces — display and search only, never resolution.
    pub slug: Option<String>,

    /// The live checkout roots, most recently used first.
    ///
    /// Empty when every recorded checkout is gone.
    pub roots: Vec<RootEntry>,
}

/// List every workspace known to the per-user `workspace/` data directory.
///
/// Scans `workspaces_dir` for user-workspace directories (`<id>` or
/// `<slug>-<id>`), deduplicates by ID (legacy layouts can hold several
/// directories for one workspace), and expands each ID to its live checkouts
/// via [`resolve_live_roots`].
/// Workspaces are ordered by most recently used checkout, newest first;
/// workspaces with no live checkout sort last.
#[must_use]
pub fn known_workspaces(workspaces_dir: &Utf8Path, storage_dir: &str) -> Vec<KnownWorkspace> {
    let Ok(entries) = workspaces_dir.read_dir_utf8() else {
        return vec![];
    };

    // Dedupe user-workspace directories by ID, preferring a named one's slug
    // over a bare one.
    let mut ids: Vec<(Id, Option<String>)> = vec![];
    for entry in entries.filter_map(std::result::Result::ok) {
        if !entry.path().is_dir() {
            continue;
        }
        let Some((id, slug)) = user_workspace_id_and_slug(entry.file_name()) else {
            continue;
        };

        match ids.iter_mut().find(|(known, _)| *known == id) {
            Some((_, known_slug)) => {
                if known_slug.is_none() {
                    *known_slug = slug;
                }
            }
            None => ids.push((id, slug)),
        }
    }

    let mut workspaces: Vec<KnownWorkspace> = ids
        .into_iter()
        .map(|(id, slug)| {
            let roots = resolve_live_roots(workspaces_dir, &id, storage_dir);
            KnownWorkspace { id, slug, roots }
        })
        .collect();

    // Most recently used first; rootless workspaces last, then by ID for a
    // stable order.
    workspaces.sort_by(|a, b| {
        let recency = |w: &KnownWorkspace| w.roots.first().map(|entry| entry.last_used);
        recency(b)
            .cmp(&recency(a))
            .then_with(|| (*a.id).cmp(&*b.id))
    });
    workspaces
}

/// Parse a user-workspace directory name into its workspace ID and optional
/// slug.
///
/// Names are `<id>` or `<slug>-<id>`, with the ID always the suffix — the same
/// rule [`matching_user_workspace_dirs`] applies when locating the directory.
/// Returns `None` for names that don't end in a well-formed ID.
fn user_workspace_id_and_slug(name: &str) -> Option<(Id, Option<String>)> {
    if let Ok(id) = name.parse::<Id>() {
        return Some((id, None));
    }

    let (slug, id) = name.rsplit_once('-')?;
    let id = id.parse::<Id>().ok()?;
    (!slug.is_empty()).then(|| (id, Some(slug.to_owned())))
}

/// The registry files in a user-workspace directory's `roots/` subdirectory.
fn registry_files(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let Ok(entries) = dir.read_dir_utf8() else {
        return vec![];
    };

    entries
        .filter_map(std::result::Result::ok)
        .map(Utf8DirEntry::into_path)
        .filter(|path| path.extension() == Some("json") && path.is_file())
        .collect()
}

/// Filesystem-safe registry key for a checkout root.
///
/// A truncated SHA-256 of the canonical path: stable across runs, and
/// collision-resistant for distinct paths, so each checkout owns exactly one
/// registry file.
fn root_key(path: &Utf8Path) -> String {
    let digest = format!("{:x}", Sha256::digest(path.as_str().as_bytes()));
    digest[..16].to_owned()
}

/// Delete a registry file, logging (not propagating) failure.
fn prune(file: &Utf8Path) {
    if let Err(error) = fs::remove_file(file) {
        warn!(%error, file = %file, "Failed to prune workspace root entry.");
    }
}

/// Remove a symlink without following it.
///
/// On Windows a directory symlink is a reparse-point directory and must be
/// removed with `remove_dir`; `remove_file` returns "Access is denied".
/// On Unix `remove_file` unlinks the symlink itself.
fn remove_symlink(link: &Utf8Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        fs::remove_dir(link)
    }
    #[cfg(not(windows))]
    {
        fs::remove_file(link)
    }
}

#[cfg(test)]
#[path = "roots_tests.rs"]
mod tests;
