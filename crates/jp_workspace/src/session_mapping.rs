//! Session-to-conversation mapping.
//!
//! Each terminal session tracks its own conversation history. The storage
//! format and file layout are managed by the [`SessionBackend`] trait; this
//! module defines the domain types and the `Workspace`-level API.
//!
//! See: `docs/rfd/020-parallel-conversations.md`
//!
//! [`SessionBackend`]: jp_storage::backend::SessionBackend

use std::{collections::HashSet, fs};

use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;
use jp_storage::backend::FsStorageBackend;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::Workspace;
use crate::session::{Session, SessionSource};

/// A session's conversation history, persisted to disk.
///
/// The `history` array tracks conversations activated in this session, ordered
/// most recent first. The active conversation is always `history[0]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionMapping {
    /// Conversation activation history, most recent first.
    pub history: Vec<SessionHistoryEntry>,

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
    /// Create a new empty mapping for the given session source.
    pub fn new(source: SessionSource) -> Self {
        Self {
            history: vec![],
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
    /// (deduplication). Otherwise it is inserted at the front.
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
    /// to `cd -` in a shell. Returns `None` if the referenced conversation no
    /// longer exists in the workspace index.
    #[must_use]
    pub fn session_previous_conversation(&self, session: &Session) -> Option<ConversationId> {
        self.load_session_mapping(session)
            .and_then(|m| m.previous_conversation_id())
            .filter(|id| self.state.conversations.contains_key(id))
    }

    /// Get all conversation IDs from the session's history.
    ///
    /// Returns IDs ordered most-recently-activated first. Only includes
    /// conversations that still exist in the workspace index.
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
                let mapping: SessionMapping = serde_json::from_value(value).ok()?;
                mapping.active_conversation_id()
            })
            .collect()
    }

    /// Record that the given session activated a conversation.
    ///
    /// Writes the session mapping to the backing store. If no mapping exists
    /// yet, one is created.
    pub fn activate_session_conversation(
        &self,
        session: &Session,
        id: ConversationId,
        now: DateTime<Utc>,
    ) -> crate::error::Result<()> {
        let mut mapping = self
            .load_session_mapping(session)
            .unwrap_or_else(|| SessionMapping::new(session.source.clone()));

        mapping.activate(id, now);

        let value = serde_json::to_value(&mapping).map_err(jp_storage::Error::from)?;
        self.sessions.save_session(session.id.as_str(), &value)?;
        Ok(())
    }

    /// Remove orphaned lock files and stale session mappings.
    ///
    /// - Lock files are orphaned if no process holds the flock.
    /// - Session mappings are stale based on the session source:
    ///   - **Getsid/Hwnd** (process liveness is checkable): the session is
    ///     deleted only when the originating process is confirmed dead. A
    ///     live process keeps its session unconditionally — we don't check
    ///     conversation existence because another process may be mid-persist
    ///     or the conversation may have been created after our index loaded.
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
            .load_all_conversation_ids()
            .into_iter()
            .collect();

        // Use filesystem-specific file listing for path-based removal.
        for path in fs.list_session_files() {
            let session_key = path.file_stem().unwrap_or_default();
            let Ok(Some(value)) = self.sessions.load_session(session_key) else {
                continue;
            };
            let Ok(mapping) = serde_json::from_value::<SessionMapping>(value) else {
                continue;
            };

            // Sources that support liveness checking (Getsid, Hwnd) are
            // authoritative: if the process is alive the session is valid,
            // regardless of whether we can see its conversations right now
            // (another process may be mid-persist, or the conversation was
            // created after our index loaded). Only delete when the process is
            // confirmed dead.
            //
            // For Env sources we can't check liveness, so we fall back to the
            // conversation-existence heuristic.
            let liveness = is_session_process_liveness(&mapping.source, session_key);

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
                drop(fs::remove_file(&path));
                continue;
            }

            // Prune individual history entries that reference deleted
            // conversations. An entry is safe to remove when the conversation
            // is absent from disk AND no other process holds its write lock
            // (which would indicate a mid-persist race).
            let original_count = mapping.history.len();
            let pruned: Vec<_> = mapping
                .history
                .iter()
                .filter(|entry| {
                    if conversation_ids.contains(&entry.id) {
                        return true;
                    }
                    // Not on disk — check if another process holds the lock. If
                    // locked, keep the entry (mid-persist). If unlocked (or no
                    // lock file), the conversation is genuinely gone.
                    self.locker.lock_info(&entry.id.to_string()).is_some()
                })
                .cloned()
                .collect();

            if pruned.len() < original_count {
                let removed = original_count - pruned.len();
                debug!(
                    path = path.to_string(),
                    removed, "Pruned dead entries from session history."
                );

                let pruned_mapping = SessionMapping {
                    history: pruned,
                    source: mapping.source,
                };

                let Ok(pruned_value) = serde_json::to_value(&pruned_mapping) else {
                    warn!(
                        path = path.to_string(),
                        "Failed to serialize pruned session mapping."
                    );
                    continue;
                };
                if let Err(error) = self.sessions.save_session(session_key, &pruned_value) {
                    warn!(
                        path = path.to_string(),
                        error = error.to_string(),
                        "Failed to rewrite session mapping after pruning."
                    );
                }
            }
        }
    }

    /// Load the session mapping for the given session.
    ///
    /// Returns `None` if no mapping exists for the session, or if the mapping
    /// cannot be parsed.
    fn load_session_mapping(&self, session: &Session) -> Option<SessionMapping> {
        match self.sessions.load_session(session.id.as_str()) {
            Ok(Some(value)) => match serde_json::from_value(value) {
                Ok(mapping) => {
                    debug!(session = session.id.as_str(), "Loaded session mapping.");
                    Some(mapping)
                }
                Err(error) => {
                    warn!(
                        session = session.id.as_str(),
                        error = error.to_string(),
                        "Failed to parse session mapping, ignoring."
                    );
                    None
                }
            },
            Ok(None) => None,
            Err(error) => {
                warn!(
                    session = session.id.as_str(),
                    error = error.to_string(),
                    "Failed to read session mapping, ignoring."
                );
                None
            }
        }
    }
}

/// Result of checking whether the session's originating process is still alive.
enum Liveness {
    /// The process is confirmed alive — do not delete the session.
    Alive,

    /// The process is confirmed dead — safe to delete.
    Dead,

    /// Liveness cannot be determined (Env sources, parse failures). Fall back
    /// to heuristics.
    Unknown,
}

/// Determine whether the process that created a session mapping is still alive.
fn is_session_process_liveness(source: &SessionSource, session_key: &str) -> Liveness {
    match source {
        SessionSource::Getsid => pid_liveness(session_key),
        SessionSource::Hwnd => hwnd_liveness(session_key),
        SessionSource::Env { .. } => Liveness::Unknown,
    }
}

/// Check whether a session leader PID is alive or dead.
///
/// See: <https://man7.org/linux/man-pages/man2/kill.2.html#:~:text=sig%20is%200>
#[cfg(unix)]
fn pid_liveness(session_key: &str) -> Liveness {
    let Ok(pid) = session_key.parse::<i32>() else {
        return Liveness::Unknown;
    };

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
fn pid_liveness(_session_key: &str) -> Liveness {
    Liveness::Unknown
}

/// Check whether a console window handle is alive or dead.
///
/// See: <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-iswindow>
#[cfg(windows)]
fn hwnd_liveness(session_key: &str) -> Liveness {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

    let Ok(hwnd) = session_key.parse::<isize>() else {
        return Liveness::Unknown;
    };

    // IsWindow returns nonzero if the handle is a valid window.
    //
    // SAFETY: IsWindow is safe to call with any handle value — it just checks
    // validity.
    if unsafe { IsWindow(hwnd as *mut core::ffi::c_void) } == 0 {
        Liveness::Dead
    } else {
        Liveness::Alive
    }
}

#[cfg(not(windows))]
fn hwnd_liveness(_session_key: &str) -> Liveness {
    Liveness::Unknown
}

#[cfg(test)]
#[path = "session_mapping_tests.rs"]
mod tests;
