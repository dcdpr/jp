//! Session-to-conversation mapping.
//!
//! Each terminal session tracks its own conversation history.
//! The storage format and file layout are managed by the [`SessionBackend`]
//! trait; this module defines the domain types and the `Workspace`-level API.
//!
//! See: `docs/rfd/020-parallel-conversations.md`
//!
//! [`SessionBackend`]: jp_storage::backend::SessionBackend

use std::{collections::HashSet, fs};

use camino::Utf8Path;
use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;
use jp_storage::backend::{ConversationFilter, FsStorageBackend};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use super::Workspace;
use crate::{
    conversation_lock::ConversationLock,
    session::{Session, SessionId, SessionSource, is_safe_path_segment, session_storage_key},
};

/// A session's conversation history, persisted to disk.
///
/// The `history` array tracks conversations activated in this session, ordered
/// most recent first.
/// The active conversation is always `history[0]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionMapping {
    /// Conversation activation history, most recent first.
    pub history: Vec<SessionHistoryEntry>,

    /// The session identity value.
    ///
    /// Recorded so the mapping is self-describing: cleanup checks process
    /// liveness from `id` + `source` without parsing the filename.
    /// Files written before this field existed backfill it from their
    /// (bare-value) filename on load.
    pub id: SessionId,

    /// How the session identity was determined.
    pub source: SessionSource,
}

/// A single entry in the session's conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionHistoryEntry {
    /// The conversation ID.
    pub id: ConversationId,

    /// When this conversation was activated in this session.
    pub activated_at: DateTime<Utc>,
}

impl SessionMapping {
    /// Create a new empty mapping for the given session.
    pub fn new(id: SessionId, source: SessionSource) -> Self {
        Self {
            history: vec![],
            id,
            source,
        }
    }

    /// The currently active conversation for this session, if any.
    pub fn active_conversation_id(&self) -> Option<ConversationId> {
        self.history.first().map(|e| e.id)
    }

    /// The previously active conversation (the one before the current).
    pub fn previous_conversation_id(&self) -> Option<ConversationId> {
        self.history.get(1).map(|e| e.id)
    }

    /// Record that a conversation was activated in this session.
    ///
    /// If the conversation was already in the history, it is moved to the front
    /// (deduplication).
    /// Otherwise it is inserted at the front.
    pub fn activate(&mut self, id: ConversationId, now: DateTime<Utc>) {
        self.history.retain(|e| e.id != id);
        self.history.insert(0, SessionHistoryEntry {
            id,
            activated_at: now,
        });
    }
}

impl Workspace {
    /// Get the active conversation ID for the given session.
    ///
    /// Returns `None` if no session mapping exists, the session is unknown, or
    /// the referenced conversation no longer exists in the workspace index.
    #[must_use]
    pub fn session_active_conversation(&self, session: &Session) -> Option<ConversationId> {
        self.load_session_mapping(session)
            .and_then(|m| m.active_conversation_id())
            .filter(|id| self.state.conversations.contains_key(id))
    }

    /// Get the previous conversation ID for the given session.
    ///
    /// This is the conversation that was active before the current one, similar
    /// to `cd -` in a shell.
    /// Returns `None` if the referenced conversation no longer exists in the
    /// workspace index.
    #[must_use]
    pub fn session_previous_conversation(&self, session: &Session) -> Option<ConversationId> {
        self.load_session_mapping(session)
            .and_then(|m| m.previous_conversation_id())
            .filter(|id| self.state.conversations.contains_key(id))
    }

    /// Get all conversation IDs from the session's history.
    ///
    /// Returns IDs ordered most-recently-activated first.
    /// Only includes conversations that still exist in the workspace index.
    #[must_use]
    pub fn session_conversation_ids(&self, session: &Session) -> Vec<ConversationId> {
        let Some(mapping) = self.load_session_mapping(session) else {
            return vec![];
        };

        mapping
            .history
            .iter()
            .map(|e| e.id)
            .filter(|id| self.state.conversations.contains_key(id))
            .collect()
    }

    /// Returns the active conversation ID from every session mapping.
    #[must_use]
    pub fn all_active_conversation_ids(&self) -> Vec<ConversationId> {
        self.sessions
            .list_session_keys()
            .into_iter()
            .filter_map(|key| {
                let value = self.sessions.load_session(&key).ok()??;
                let mapping = mapping_from_value(value, &key)?;
                mapping.active_conversation_id()
            })
            .collect()
    }

    /// Record that the given session activated a conversation.
    ///
    /// Writes only the session mapping.
    /// Use [`activate_session_conversation`] when you hold a
    /// [`ConversationLock`] — that variant also bumps the conversation's
    /// `last_activated_at`, which is the canonical "this conversation was last
    /// worked on" timestamp consumed by sorting, archiving, and `--id=last`.
    ///
    /// This bare form is for the lock-contention fallback in `jp c use` (where
    /// another process holds the lock, so we can't write the metadata, and
    /// accept that `last_activated_at` will reflect the lock holder's
    /// activation time rather than ours) and for tests that exercise
    /// session-mapping logic in isolation.
    ///
    /// [`activate_session_conversation`]: Self::activate_session_conversation
    pub fn record_session_activation(
        &self,
        session: &Session,
        id: ConversationId,
        now: DateTime<Utc>,
    ) -> crate::error::Result<()> {
        let (key, mut mapping) = self.load_session_entry(session).unwrap_or_else(|| {
            (
                session.storage_key(),
                SessionMapping::new(session.id.clone(), session.source.clone()),
            )
        });

        mapping.activate(id, now);

        let value = serde_json::to_value(&mapping).map_err(jp_storage::Error::from)?;
        // Write to the key the mapping already lives at, so reads and writes
        // within an invocation stay on one file; cleanup owns migrating a
        // legacy bare-value file to its source-prefixed name.
        self.sessions.save_session(&key, &value)?;
        Ok(())
    }

    /// Record that a conversation was activated in the given session.
    ///
    /// Updates two things:
    ///
    /// 1. The session-to-conversation mapping (most recent first), via
    ///    [`record_session_activation`].
    /// 2. The conversation's `last_activated_at` timestamp on its metadata, via
    ///    the held lock.
    ///    The metadata write is staged in-memory and flushes when the resulting
    ///    [`ConversationMut`] drops at the end of this call.
    ///
    /// Requiring `&ConversationLock` makes it a type-level invariant that the
    /// caller holds the conversation's exclusive lock — so the metadata bump
    /// is safe to perform.
    ///
    /// [`ConversationMut`]: crate::ConversationMut
    /// [`record_session_activation`]: Self::record_session_activation
    pub fn activate_session_conversation(
        &self,
        conv: &ConversationLock,
        session: &Session,
        now: DateTime<Utc>,
    ) -> crate::error::Result<()> {
        conv.as_mut().update_metadata(|m| m.last_activated_at = now);
        self.record_session_activation(session, conv.id(), now)
    }

    /// Remove orphaned lock files and stale session mappings.
    ///
    /// - Lock files are orphaned if no process holds the flock.
    /// - Session mappings are stale based on the session source:
    ///   - **Getsid/Hwnd** (process liveness is checkable): the session is
    ///     deleted only when the originating process is confirmed dead.
    ///     A live process keeps its session unconditionally — we don't check
    ///     conversation existence because another process may be mid-persist or
    ///     the conversation may have been created after our index loaded.
    ///   - **Env** (liveness unknown): falls back to conversation existence.
    ///     If none of the conversations in the history exist on disk, the
    ///     mapping is removed.
    pub fn cleanup_stale_files(&self, fs: Option<&FsStorageBackend>) {
        let Some(fs) = fs else {
            return;
        };

        // Remove orphaned lock files (filesystem-specific: needs file paths).
        for path in fs.list_orphaned_lock_files() {
            debug!(path = %path, "Removing orphaned lock file.");
            drop(fs::remove_file(&path));
        }

        // Remove stale session mappings.
        //
        // Re-scan conversation IDs from disk instead of using the in-memory
        // state. Other processes may have created conversations after our index
        // was loaded at startup; using the stale snapshot would incorrectly
        // mark their sessions as having "no live conversations" and delete
        // them.
        let conversation_ids: HashSet<_> = self
            .loader
            .load_conversation_ids(ConversationFilter::default())
            .into_iter()
            .collect();

        // Use filesystem-specific file listing for path-based removal.
        for path in fs.list_session_files() {
            self.cleanup_session_file(&path, &conversation_ids);
        }
    }

    /// Evaluate a single session mapping file: remove it if stale, prune dead
    /// history entries, and migrate a legacy bare-value filename to its
    /// source-prefixed key.
    fn cleanup_session_file(&self, path: &Utf8Path, conversation_ids: &HashSet<ConversationId>) {
        let session_key = path.file_stem().unwrap_or_default();
        let Ok(Some(value)) = self.sessions.load_session(session_key) else {
            return;
        };
        let Some(mapping) = mapping_from_value(value, session_key) else {
            return;
        };

        // Sources that support liveness checking (Getsid, Hwnd) are
        // authoritative: if the process is alive the session is valid,
        // regardless of whether we can see its conversations right now (another
        // process may be mid-persist, or the conversation was created after our
        // index loaded). Only delete when the process is confirmed dead.
        //
        // For Env sources we can't check liveness, so we fall back to the
        // conversation-existence heuristic. The liveness value is decoded from
        // the mapping's id, not the filename, so the source-prefixed naming
        // below doesn't disturb it.
        let liveness = is_session_process_liveness(&mapping.id, &mapping.source);

        let should_remove = match liveness {
            Liveness::Alive => false,
            Liveness::Dead => {
                debug!(
                    path = path.to_string(),
                    source = mapping.source.to_string(),
                    "Removing stale session mapping (process dead)."
                );
                true
            }
            Liveness::Unknown => {
                let has_live = mapping.history.iter().any(|entry| {
                    conversation_ids.contains(&entry.id)
                        || self.locker.lock_info(&entry.id.to_string()).is_some()
                });

                if !has_live {
                    debug!(
                        path = path.to_string(),
                        "Removing stale session mapping (no live conversations)."
                    );
                }
                !has_live
            }
        };

        if should_remove {
            drop(fs::remove_file(path));
            return;
        }

        // A legacy bare-value file is migrated to its source-prefixed key.
        let desired_key = session_storage_key(&mapping.id, &mapping.source);
        let needs_migration = session_key != desired_key;

        // A file already present at the destination means this legacy file is a
        // leftover from a partial migration; drop it rather than clobbering the
        // current file with stale history.
        if needs_migration && matches!(self.sessions.load_session(&desired_key), Ok(Some(_))) {
            debug!(
                path = path.to_string(),
                "Removing duplicate legacy session mapping."
            );
            drop(fs::remove_file(path));
            return;
        }

        // Prune individual history entries that reference deleted conversations.
        // An entry is safe to remove when the conversation is absent from disk
        // AND no other process holds its write lock (which would indicate a
        // mid-persist race).
        let original_count = mapping.history.len();
        let pruned: Vec<_> = mapping
            .history
            .iter()
            .filter(|entry| {
                if conversation_ids.contains(&entry.id) {
                    return true;
                }
                // Not on disk — check if another process holds the lock. If
                // locked, keep the entry (mid-persist). If unlocked (or no lock
                // file), the conversation is genuinely gone.
                self.locker.lock_info(&entry.id.to_string()).is_some()
            })
            .cloned()
            .collect();

        let pruned_changed = pruned.len() < original_count;
        if pruned_changed {
            debug!(
                path = path.to_string(),
                removed = original_count - pruned.len(),
                "Pruned dead entries from session history."
            );
        }

        // Nothing to write unless an entry was pruned or the file needs to move
        // to its source-prefixed name.
        if !pruned_changed && !needs_migration {
            return;
        }

        let updated = SessionMapping {
            history: pruned,
            id: mapping.id,
            source: mapping.source,
        };
        let Ok(updated_value) = serde_json::to_value(&updated) else {
            warn!(
                path = path.to_string(),
                "Failed to serialize session mapping."
            );
            return;
        };
        if let Err(error) = self.sessions.save_session(&desired_key, &updated_value) {
            warn!(
                path = path.to_string(),
                error = error.to_string(),
                "Failed to rewrite session mapping."
            );
            return;
        }

        if needs_migration {
            debug!(
                path = path.to_string(),
                key = desired_key,
                "Migrated session mapping to source-prefixed key."
            );
            drop(fs::remove_file(path));
        }
    }

    /// Load the session mapping for the given session.
    ///
    /// Returns `None` if no mapping exists for the session, or if the mapping
    /// cannot be parsed.
    fn load_session_mapping(&self, session: &Session) -> Option<SessionMapping> {
        self.load_session_entry(session).map(|(_, mapping)| mapping)
    }

    /// Load the session mapping together with the storage key it lives at.
    ///
    /// Resolves the source-prefixed key first, then falls back to the legacy
    /// bare-value key for files written before the source was encoded into the
    /// filename.
    /// The returned key is where a subsequent write should land so it updates
    /// the existing file rather than forking a duplicate.
    fn load_session_entry(&self, session: &Session) -> Option<(String, SessionMapping)> {
        let key = session.storage_key();
        if let Some(mapping) = self.read_matching_mapping(&key, session) {
            return Some((key, mapping));
        }

        // Legacy fallback for files written before the source was encoded into
        // the filename. The bare value was the filename, so only probe it when
        // it is a safe single path segment — an env value like `x/../../outside`
        // must not be interpreted as path components on this compat read.
        let legacy_key = session.id.as_str();
        if legacy_key != key
            && is_safe_path_segment(legacy_key)
            && let Some(mapping) = self.read_matching_mapping(legacy_key, session)
        {
            debug!(legacy = legacy_key, "Loaded legacy session mapping.");
            return Some((legacy_key.to_owned(), mapping));
        }

        None
    }

    /// Read the mapping at `key`, returning it only when its stored identity
    /// matches `session`.
    ///
    /// `storage_key` is not guaranteed injective — sanitized env keys can
    /// alias (`env("A-B")` and `env("A_B")`), a pre-fix bare-value file is
    /// shared across sources, and hashes can (astronomically) collide.
    /// So the stored `id` + `source` is the authority for ownership, never the
    /// filename; this stops one session from adopting another's history through
    /// a key clash.
    fn read_matching_mapping(&self, key: &str, session: &Session) -> Option<SessionMapping> {
        let mapping = self.read_session_mapping(key)?;
        (mapping.id == session.id && mapping.source == session.source).then_some(mapping)
    }

    /// Read and parse the session mapping stored under `key`.
    fn read_session_mapping(&self, key: &str) -> Option<SessionMapping> {
        match self.sessions.load_session(key) {
            Ok(Some(value)) => {
                let mapping = mapping_from_value(value, key);
                if mapping.is_none() {
                    warn!(session = key, "Failed to parse session mapping, ignoring.");
                }
                mapping
            }
            Ok(None) => None,
            Err(error) => {
                warn!(
                    session = key,
                    error = error.to_string(),
                    "Failed to read session mapping, ignoring."
                );
                None
            }
        }
    }
}

/// Deserialize a session mapping, backfilling `id` for the legacy on-disk
/// layout that stored the session value only in the filename.
///
/// `fallback_id` is the key the value was loaded under; for a legacy file that
/// key is the bare session value, which is exactly the missing `id`.
fn mapping_from_value(mut value: Value, fallback_id: &str) -> Option<SessionMapping> {
    if let Some(object) = value.as_object_mut() {
        object
            .entry("id")
            .or_insert_with(|| Value::String(fallback_id.to_owned()));
    }

    serde_json::from_value(value).ok()
}

/// Result of checking whether the session's originating process is still alive.
enum Liveness {
    /// The process is confirmed alive — do not delete the session.
    Alive,

    /// The process is confirmed dead — safe to delete.
    Dead,

    /// Liveness cannot be determined (Env sources, parse failures).
    /// Fall back to heuristics.
    Unknown,
}

/// Determine whether the process that created a session mapping is still alive.
///
/// The id is decoded into the typed handle via `SessionId::as_pid` /
/// [`SessionId::as_hwnd`] — the inverse of the encoding in [`Session::getsid`]
/// / [`Session::hwnd`].
/// A non-decodable id (or `Env` source) yields `Unknown`.
fn is_session_process_liveness(id: &SessionId, source: &SessionSource) -> Liveness {
    match source {
        SessionSource::Getsid => id.as_pid().map_or(Liveness::Unknown, pid_liveness),
        SessionSource::Hwnd => id.as_hwnd().map_or(Liveness::Unknown, hwnd_liveness),
        SessionSource::Env { .. } => Liveness::Unknown,
    }
}

/// Check whether a session leader PID is alive or dead.
///
/// See:
/// <https://man7.org/linux/man-pages/man2/kill.2.html#:~:text=sig%20is%200>
#[cfg(unix)]
fn pid_liveness(pid: i32) -> Liveness {
    // kill(pid, 0) checks if a process exists without sending a signal. Returns
    // 0 if the process exists (and we have permission to signal it), or -1 with
    // ESRCH if it doesn't exist.
    //
    // SAFETY: kill with signal 0 is a standard POSIX operation that only checks
    // process existence. The PID comes from a session mapping file that was
    // written by a previous JP invocation.
    let ret = unsafe { libc::kill(pid, 0) };
    if ret == 0 {
        return Liveness::Alive;
    }

    // ESRCH = no such process. Any other errno (e.g. EPERM) means the process
    // exists but we can't signal it — treat as alive.
    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Liveness::Dead
    } else {
        Liveness::Alive
    }
}

#[cfg(not(unix))]
fn pid_liveness(_pid: i32) -> Liveness {
    Liveness::Unknown
}

/// Check whether a console window handle is alive or dead.
///
/// See:
/// <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-iswindow>
#[cfg(windows)]
fn hwnd_liveness(handle: isize) -> Liveness {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

    // IsWindow returns nonzero if the handle is a valid window.
    //
    // SAFETY: IsWindow is safe to call with any handle value — it just checks
    // validity.
    if unsafe { IsWindow(handle as *mut core::ffi::c_void) } == 0 {
        Liveness::Dead
    } else {
        Liveness::Alive
    }
}

#[cfg(not(windows))]
fn hwnd_liveness(_handle: isize) -> Liveness {
    Liveness::Unknown
}

#[cfg(test)]
#[path = "session_mapping_tests.rs"]
mod tests;
