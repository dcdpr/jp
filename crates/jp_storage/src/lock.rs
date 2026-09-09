//! Advisory file-based conversation locks.
//!
//! The domain layer over [`resource_lock`]: conversation-id resource names,
//! [`LockInfo`] holder diagnostics, and the user-vs-workspace lock-file
//! placement policy.
//! The locking mechanics themselves (OS advisory locks, guard lifetimes) live
//! in [`resource_lock`].
//!
//! [`resource_lock`]: crate::resource_lock

use std::fs::File;

use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use relative_path::RelativePath;
use serde::{Deserialize, Serialize};

use crate::{
    error::Result,
    resource_lock::{FsResourceLocker, ResourceGuard, ResourceLocker as _, try_exclusive_lock},
};

pub(crate) const LOCKS_DIR: &str = "locks";

/// Diagnostic metadata written to the lock file.
///
/// This is informational only — the actual locking is done by the OS via
/// `flock`/`LockFileEx`.
/// If the process is killed with SIGKILL, the metadata may be stale but the OS
/// releases the lock automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    /// PID of the process that holds the lock.
    pub pid: u32,

    /// Session identity of the lock holder (if known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,

    /// When the lock was acquired.
    pub acquired_at: String,
}

/// Read diagnostic info from a lock file (best-effort).
///
/// Returns `None` if the file can't be read or parsed.
#[must_use]
pub fn read_lock_info(path: &Utf8Path) -> Option<LockInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

impl super::Storage {
    /// Try to acquire an exclusive lock on a conversation.
    ///
    /// `session` is the current session identity, written to the lock file for
    /// diagnostic purposes.
    ///
    /// The lock file is placed in user storage if available, otherwise in
    /// workspace storage.
    ///
    /// Returns `Ok(Some(guard))` if the lock was acquired, `Ok(None)` if
    /// another process holds it, or `Err` on I/O errors.
    pub fn try_lock_conversation(
        &self,
        conversation_id: &str,
        session: Option<&str>,
    ) -> Result<Option<Box<dyn ResourceGuard>>> {
        let path = self.lock_file_path(conversation_id).unwrap_or_else(|p| p);
        let dir = path.parent().unwrap_or(Utf8Path::new("."));

        // Holder info is purely diagnostic; failing to serialize it must
        // not fail the acquisition.
        let info = serde_json::to_string(&LockInfo {
            pid: std::process::id(),
            session: session.map(String::from),
            acquired_at: Utc::now().to_rfc3339(),
        })
        .ok();

        // Conversation lock files double as presence markers (see
        // `is_conversation_locked` and the orphan scan), so they are removed
        // when the guard drops. Removal is not free: a contender that opens the
        // path between the handle closing and the unlink locks an inode about
        // to disappear, while the next acquisition locks a fresh file at the
        // same path. `try_lock` keeps that to the close/unlink window instead
        // of the certainty a blocking wait would give, but does not close it;
        // RFD 106 tracks the fix as the release-then-unlink defect.
        FsResourceLocker::new(dir)
            .with_remove_on_drop()
            .try_lock(conversation_id, info.as_deref())
            .map_err(|error| error.source.into())
    }

    /// Read lock holder info for a conversation.
    ///
    /// Returns `None` if there's no lock file or the file can't be parsed.
    /// Checks user storage first, then workspace storage.
    #[must_use]
    pub fn read_conversation_lock_info(&self, conversation_id: &str) -> Option<LockInfo> {
        let path = self.lock_file_path(conversation_id).ok()?;
        read_lock_info(&path)
    }

    /// Check whether a conversation is currently locked by another process.
    ///
    /// Returns `true` if a lock file exists and is held (not orphaned).
    /// Returns `false` if no lock file exists or the lock is orphaned.
    #[must_use]
    pub fn is_conversation_locked(&self, conversation_id: &str) -> bool {
        match self.lock_file_path(conversation_id) {
            Ok(path) => !is_orphaned_lock(&path),
            Err(_) => false,
        }
    }

    /// Resolve the lock file path for a conversation.
    ///
    /// Returns `Ok(path)` if a lock file already exists (checking user storage
    /// first, then workspace storage), or `Err(path)` with the preferred write
    /// location if no lock file exists.
    fn lock_file_path(
        &self,
        conversation_id: &str,
    ) -> std::result::Result<Utf8PathBuf, Utf8PathBuf> {
        let locks_path = RelativePath::new(LOCKS_DIR);
        let name = format!("{conversation_id}.lock");
        let preferred = self.user_or_root_with_path(locks_path).join(&name);

        if preferred.exists() {
            return Ok(preferred);
        }

        // Check the other location if user storage is configured.
        if self.user.is_some() {
            let fallback = self.root_with_path(locks_path).join(&name);
            if fallback.exists() {
                return Ok(fallback);
            }
        }

        Err(preferred)
    }
}

/// Check whether a lock file is orphaned (no process holds the lock).
///
/// Opens the file, attempts a non-blocking exclusive lock.
/// If it succeeds, the file is orphaned.
/// The lock is immediately released.
pub(crate) fn is_orphaned_lock(path: &camino::Utf8Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };

    if try_exclusive_lock(&file) {
        // We acquired the lock, meaning nobody else holds it. Release
        // immediately by dropping the file.
        true
    } else {
        false
    }
}
