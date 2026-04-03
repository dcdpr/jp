//! Session-to-conversation mapping.
//!
//! Each terminal session tracks its own conversation history. The storage
//! format and file layout are managed by [`jp_storage::Storage`]; this module
//! defines the domain types and the `Workspace`-level API.
//!
//! See: `docs/rfd/020-parallel-conversations.md`

use std::{collections::HashSet, fs};

use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;
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
    /// user storage is not configured.
    #[must_use]
    pub fn session_active_conversation(&self, session: &Session) -> Option<ConversationId> {
        self.load_session_mapping(session)
            .and_then(|m| m.active_conversation_id())
    }

    /// Get the previous conversation ID for the given session.
    ///
    /// This is the conversation that was active before the current one, similar
    /// to `cd -` in a shell.
    #[must_use]
    pub fn session_previous_conversation(&self, session: &Session) -> Option<ConversationId> {
        self.load_session_mapping(session)
            .and_then(|m| m.previous_conversation_id())
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
        let Some(storage) = self.storage.as_ref() else {
            return vec![];
        };

        storage
            .list_session_files()
            .into_iter()
            .filter_map(|path| {
                let key = path.file_stem()?;
                let mapping: SessionMapping = storage.load_session_data(key).ok()??;
                mapping.active_conversation_id()
            })
            .collect()
    }

    /// Record that the given session activated a conversation.
    ///
    /// Writes the session mapping to disk. If no mapping exists yet, one is
    /// created.
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

        let storage = self.storage.as_ref().ok_or(crate::Error::MissingStorage)?;
        storage.save_session_data(session.id.as_str(), &mapping)?;
        Ok(())
    }

    /// Remove orphaned lock files and stale session mappings.
    ///
    /// - Lock files are orphaned if no process holds the flock.
    /// - Session mappings are stale based on two criteria:
    ///   1. **Process liveness** (Getsid/Hwnd sources): if the session leader
    ///      process is no longer alive, the mapping is stale regardless of
    ///      whether its conversations still exist.
    ///   2. **Conversation existence** (all sources): if none of the
    ///      conversations in the mapping's history exist in the workspace, the
    ///      mapping is stale.
    pub fn cleanup_stale_files(&self) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };

        // Remove orphaned lock files.
        for path in storage.list_orphaned_lock_files() {
            debug!(path = %path, "Removing orphaned lock file.");
            drop(fs::remove_file(&path));
        }

        // Remove stale session mappings.
        let conversation_ids: HashSet<_> = self.conversations().map(|(id, _)| *id).collect();

        for path in storage.list_session_files() {
            let session_key = path.file_stem().unwrap_or_default();
            let Ok(Some(mapping)) = storage.load_session_data::<SessionMapping>(session_key) else {
                continue;
            };

            // Check process liveness for sources that support it.
            if is_session_process_dead(&mapping.source, session_key) {
                debug!(
                    path = path.to_string(),
                    source = mapping.source.to_string(),
                    "Removing stale session mapping (process dead)."
                );

                drop(fs::remove_file(&path));
                continue;
            }

            // For all sources: remove if no referenced conversation exists.
            let has_live = mapping
                .history
                .iter()
                .any(|entry| conversation_ids.contains(&entry.id));

            if !has_live {
                debug!(
                    path = path.to_string(),
                    "Removing stale session mapping (no live conversations)."
                );

                drop(fs::remove_file(&path));
            }
        }
    }

    /// Load the session mapping for the given session.
    ///
    /// Returns `None` if there is no storage, no user storage, no mapping file,
    /// or the file cannot be parsed.
    fn load_session_mapping(&self, session: &Session) -> Option<SessionMapping> {
        let storage = self.storage.as_ref()?;

        match storage.load_session_data(session.id.as_str()) {
            Ok(Some(mapping)) => {
                debug!(session = session.id.as_str(), "Loaded session mapping.");
                Some(mapping)
            }
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

/// Check whether the process that created a session mapping is dead.
///
/// Returns `true` if the source supports liveness checking and the process is
/// no longer alive. Returns `false` if liveness cannot be determined (Env
/// sources, parse failures, unsupported platforms).
fn is_session_process_dead(source: &SessionSource, session_key: &str) -> bool {
    match source {
        SessionSource::Getsid => is_pid_dead(session_key),
        SessionSource::Hwnd => is_hwnd_dead(session_key),
        SessionSource::Env { .. } => false,
    }
}

/// Check whether a PID is dead by sending signal 0.
///
/// See: <https://man7.org/linux/man-pages/man2/kill.2.html#:~:text=sig%20is%200>
#[cfg(unix)]
fn is_pid_dead(session_key: &str) -> bool {
    let Ok(pid) = session_key.parse::<i32>() else {
        return false;
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
        return false; // process exists
    }

    // ESRCH = no such process. Any other errno (e.g. EPERM) means the
    // process exists but we can't signal it — treat as alive.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn is_pid_dead(_session_key: &str) -> bool {
    false
}

/// Check whether a console window handle is dead.
///
/// On Windows, check if the window handle is still valid. On other platforms,
/// return false (can't check).
///
/// See: <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-iswindow>
#[cfg(windows)]
fn is_hwnd_dead(session_key: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

    let Ok(hwnd) = session_key.parse::<isize>() else {
        return false;
    };

    // IsWindow returns nonzero if the handle is a valid window.
    //
    // SAFETY: IsWindow is safe to call with any handle value — it just checks
    // validity.
    unsafe { IsWindow(hwnd as *mut core::ffi::c_void) == 0 }
}

#[cfg(not(windows))]
fn is_hwnd_dead(_session_key: &str) -> bool {
    false
}

#[cfg(test)]
#[path = "session_mapping_tests.rs"]
mod tests;
