//! User-global session → active-workspace store (RFD 087).
//!
//! Maps a terminal session to the workspace it drives, *above* any
//! user-workspace directory:
//!
//! ```text
//! <user-data-dir>/sessions/<source-key>.json
//! ```
//!
//! The store mirrors the per-workspace session-to-conversation mapping (RFD
//! 020, [`session_mapping`]): a most-recent-first `history` of selections
//! (active = `history[0]`, previous = `history[1]`), the session identity for
//! stale detection, and a session-level `sticky` flag (the precedence ladder's
//! persisted `A` choice).
//! Filenames reuse [`Session::storage_key`], so an automatic `getsid` / `Hwnd`
//! session can never alias an `Env` session sharing the same numeric value.
//!
//! Each entry records the workspace ID *and* the resolved checkout root:
//! distinct checkouts of one workspace are distinct history entries, and the ID
//! is what makes recovery possible after a recorded root is deleted.
//!
//! See: `docs/rfd/087-session-scoped-active-workspace.md`
//!
//! [`session_mapping`]: crate::session_mapping

use std::{fs, str::FromStr as _};

use camino::{Utf8DirEntry, Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use jp_storage::value::{read_json, write_json};
use serde::{Deserialize, Serialize};
use tracing::{debug, trace, warn};

use crate::{
    Id,
    error::Result,
    session::{Session, SessionId, SessionSource},
    session_mapping::{Liveness, is_session_process_liveness},
};

/// Directory under the user data directory holding the per-session records.
pub const SESSIONS_DIR: &str = "sessions";

/// A single selected workspace in a session's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSelection {
    /// The selected workspace ID.
    ///
    /// Stored alongside the root so recovery can expand the ID through the
    /// roots registry once the recorded root is gone — at that point the ID
    /// can no longer be read from `<root>/.jp/.id`.
    pub workspace_id: String,

    /// The concrete checkout root the selection resolved to.
    pub root: Utf8PathBuf,

    /// When this selection was recorded.
    pub selected_at: DateTime<Utc>,
}

impl WorkspaceSelection {
    /// The entry's workspace ID, when it parses as one.
    ///
    /// A tampered or corrupted record can hold anything; callers treat an
    /// unparseable ID as a dead entry.
    #[must_use]
    pub fn id(&self) -> Option<Id> {
        Id::from_str(&self.workspace_id).ok()
    }
}

/// The on-disk shape of one session's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSessionMapping {
    /// Most-recent-first history of selected workspaces.
    ///
    /// The active workspace is `history[0]`; the previously active one — the
    /// `session` / `s` target — is `history[1]`.
    pub history: Vec<WorkspaceSelection>,

    /// Whether the session keeps using its active workspace even when the cwd
    /// resolves to a different one.
    ///
    /// The persisted `A` choice from the precedence ladder; interactive-only
    /// state, cleared by `jp w use cwd`.
    #[serde(default)]
    pub sticky: bool,

    /// The session identity value, for stale detection.
    pub id: SessionId,

    /// How the session identity was resolved; drives the cleanup rule split.
    pub source: SessionSource,
}

/// The user-global store of session → active-workspace records.
///
/// Owned by the `jp_cli` bootstrap step, not by [`Workspace`]: records exist
/// before any workspace is selected and reference workspace IDs the current run
/// never touches.
///
/// [`Workspace`]: crate::Workspace
#[derive(Debug, Clone)]
pub struct WorkspaceSessionStore {
    /// The `sessions/` directory holding one JSON file per session.
    dir: Utf8PathBuf,
}

impl WorkspaceSessionStore {
    /// A store rooted at the given `sessions/` directory.
    pub fn new(dir: impl Into<Utf8PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// A store at its standard location under the user data directory.
    #[must_use]
    pub fn at_user_data_dir(data_dir: &Utf8Path) -> Self {
        Self::new(data_dir.join(SESSIONS_DIR))
    }

    /// The record file for `session`.
    fn path(&self, session: &Session) -> Utf8PathBuf {
        self.dir.join(format!("{}.json", session.storage_key()))
    }

    /// Load the record for `session`, if any.
    ///
    /// Only a record matching the full session identity is honored: the env
    /// value in the filename is hashed, so a (however unlikely) collision or a
    /// tampered store must not hand a foreign selection to this session.
    #[must_use]
    pub fn load(&self, session: &Session) -> Option<WorkspaceSessionMapping> {
        let mapping: WorkspaceSessionMapping = read_json(&self.path(session)).ok()?;
        (mapping.id == session.id && mapping.source == session.source).then_some(mapping)
    }

    /// The session's active workspace selection (`history[0]`).
    #[must_use]
    pub fn active(&self, session: &Session) -> Option<WorkspaceSelection> {
        self.load(session)?.history.into_iter().next()
    }

    /// The session's previously active selection (`history[1]`), the `session`
    /// / `s` target.
    #[must_use]
    pub fn previous(&self, session: &Session) -> Option<WorkspaceSelection> {
        self.load(session)?.history.into_iter().nth(1)
    }

    /// Record a workspace selection as the session's active workspace.
    ///
    /// The (workspace, checkout) pair moves to the front of the history;
    /// distinct checkouts of the same workspace ID stay distinct entries (`s`
    /// restores the exact previously active checkout, like `cd -`).
    /// The session-level `sticky` flag is preserved.
    pub fn record_selection(
        &self,
        session: &Session,
        workspace_id: &Id,
        root: &Utf8Path,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let mut mapping = self
            .load(session)
            .unwrap_or_else(|| WorkspaceSessionMapping {
                history: vec![],
                sticky: false,
                id: session.id.clone(),
                source: session.source.clone(),
            });

        mapping
            .history
            .retain(|entry| !(entry.workspace_id == **workspace_id && entry.root == root));
        mapping.history.insert(0, WorkspaceSelection {
            workspace_id: workspace_id.to_string(),
            root: root.to_owned(),
            selected_at: now,
        });

        write_json(&self.path(session), &mapping)?;
        trace!(
            workspace = %workspace_id,
            root = %root,
            "Recorded session-active workspace selection."
        );
        Ok(())
    }

    /// Persist the session's `sticky` flag (the precedence ladder's `A`
    /// choice).
    ///
    /// A sticky session keeps using its active workspace even when the cwd
    /// resolves to a different one, until `jp w use cwd` clears the record.
    /// Without a record there is no selection to pin, so the call is a no-op.
    pub fn set_sticky(&self, session: &Session, sticky: bool) -> Result<()> {
        let Some(mut mapping) = self.load(session) else {
            debug!("No session-active workspace recorded; nothing to pin.");
            return Ok(());
        };

        if mapping.sticky == sticky {
            return Ok(());
        }

        mapping.sticky = sticky;
        write_json(&self.path(session), &mapping)?;
        trace!(sticky, "Updated the session's sticky flag.");
        Ok(())
    }

    /// Drop the session's record (`jp w use cwd`).
    ///
    /// Clearing returns the session to cwd resolution; the record — history
    /// and sticky flag included — is removed.
    pub fn clear(&self, session: &Session) -> Result<()> {
        match fs::remove_file(self.path(session)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Source-split cleanup pass over every record (RFD 087).
    ///
    /// - **Getsid / Hwnd** (process liveness is checkable): the record is
    ///   removed only when the originating process is confirmed dead; a live
    ///   process keeps its record unconditionally.
    /// - **Env** (liveness unknown): existence-based across the whole history.
    ///   An entry is pruned only when its workspace ID has no live root — not
    ///   merely when its recorded root died, which is what lets missing-root
    ///   recovery re-prompt among the ID's surviving checkouts.
    ///   The record is removed only when no entry references a workspace ID
    ///   with any live root.
    ///
    /// `workspace_has_live_root` answers "does this workspace ID have at least
    /// one live checkout?"; the caller supplies it so the store stays agnostic
    /// of where roots registries live.
    pub fn cleanup(&self, workspace_has_live_root: &dyn Fn(&Id) -> bool) {
        for file in record_files(&self.dir) {
            cleanup_record(&file, workspace_has_live_root);
        }
    }
}

/// Evaluate a single session record file against the cleanup rules.
fn cleanup_record(path: &Utf8Path, workspace_has_live_root: &dyn Fn(&Id) -> bool) {
    let mapping = match read_json::<WorkspaceSessionMapping>(path) {
        Ok(mapping) => mapping,
        Err(error) => {
            // Unlike the per-workspace store, this store has no legacy
            // formats: a record it cannot read can never become readable.
            warn!(%error, file = %path, "Pruning unreadable workspace session record.");
            prune(path);
            return;
        }
    };

    match is_session_process_liveness(&mapping.id, &mapping.source) {
        // A live process keeps its record unconditionally.
        Liveness::Alive => {}
        Liveness::Dead => {
            debug!(
                file = %path,
                source = %mapping.source,
                "Removing stale workspace session record (process dead)."
            );
            prune(path);
        }
        Liveness::Unknown => {
            let live: Vec<_> = mapping
                .history
                .iter()
                .filter(|entry| entry.id().is_some_and(|id| workspace_has_live_root(&id)))
                .cloned()
                .collect();

            if live.is_empty() {
                debug!(
                    file = %path,
                    "Removing stale workspace session record (no live workspaces)."
                );
                prune(path);
                return;
            }

            if live.len() < mapping.history.len() {
                let updated = WorkspaceSessionMapping {
                    history: live,
                    ..mapping
                };
                if let Err(error) = write_json(path, &updated) {
                    warn!(%error, file = %path, "Failed to rewrite workspace session record.");
                }
            }
        }
    }
}

/// The record files in the store's directory.
fn record_files(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let Ok(entries) = dir.read_dir_utf8() else {
        return vec![];
    };

    entries
        .filter_map(std::result::Result::ok)
        .map(Utf8DirEntry::into_path)
        .filter(|path| path.extension() == Some("json") && path.is_file())
        .collect()
}

/// Delete a record file, logging (not propagating) failure.
fn prune(file: &Utf8Path) {
    if let Err(error) = fs::remove_file(file) {
        warn!(%error, file = %file, "Failed to prune workspace session record.");
    }
}

#[cfg(test)]
#[path = "session_store_tests.rs"]
mod tests;
