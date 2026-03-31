//! Advisory file-based conversation locks.
//!
//! Uses OS-level advisory locks (`flock` on Unix, `LockFileEx` on Windows)
//! to prevent concurrent writes to the same conversation.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::error::Result;

pub(crate) const LOCKS_DIR: &str = "locks";

/// Diagnostic metadata written to the lock file.
///
/// This is informational only — the actual locking is done by the OS via
/// `flock`/`LockFileEx`. If the process is killed with SIGKILL, the metadata
/// may be stale but the OS releases the lock automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    /// PID of the process that holds the lock.
    pub pid: u32,
    /// Session identity of the lock holder (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// When the lock was acquired.
    pub acquired_at: String,
}

/// An acquired exclusive advisory lock on a conversation.
///
/// The OS lock is held as long as the `File` is open. On drop, the lock file
/// is deleted and the file handle is closed (releasing the flock).
#[derive(Debug)]
pub struct ConversationFileLock {
    file: Option<File>,
    path: Utf8PathBuf,
}

impl ConversationFileLock {
    /// Try to acquire an exclusive advisory lock on the given path.
    ///
    /// `session` is the current process's session identity, written to the
    /// lock file for diagnostics.
    ///
    /// Returns `Ok(Some(lock))` if the lock was acquired, `Ok(None)` if
    /// another process holds it, or `Err` on I/O failure.
    fn try_acquire(path: Utf8PathBuf, session: Option<&str>) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&path)?;

        if try_exclusive_lock(&file) {
            let mut lock = Self {
                file: Some(file),
                path,
            };
            lock.write_info(session);
            Ok(Some(lock))
        } else {
            Ok(None)
        }
    }

    /// Write diagnostic info to the lock file (best-effort).
    fn write_info(&mut self, session: Option<&str>) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let info = LockInfo {
            pid: std::process::id(),
            session: session.map(String::from),
            acquired_at: chrono::Utc::now().to_rfc3339(),
        };
        // Truncate + rewrite. Ignore errors — the info is purely diagnostic.
        drop(file.set_len(0));
        drop(file.seek(std::io::SeekFrom::Start(0)));
        drop(serde_json::to_writer(&*file, &info));
        drop(file.flush());
    }
}

/// Read diagnostic info from a lock file (best-effort).
///
/// Returns `None` if the file can't be read or parsed.
#[must_use]
pub fn read_lock_info(path: &Utf8Path) -> Option<LockInfo> {
    let mut file = File::open(path).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    serde_json::from_str(&buf).ok()
}

impl Drop for ConversationFileLock {
    fn drop(&mut self) {
        // Drop the file handle first to release the OS lock.
        self.file.take();
        // Best-effort cleanup of the lock file.
        drop(fs::remove_file(&self.path));
    }
}

impl ConversationFileLock {
    /// Create a no-op lock for test environments without on-disk storage.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    pub fn test_noop() -> Self {
        Self {
            file: None,
            path: Utf8PathBuf::from("/dev/null"),
        }
    }
}

impl super::Storage {
    /// Try to acquire an exclusive lock on a conversation.
    ///
    /// `session` is the current session identity, written to the lock file
    /// for diagnostic purposes.
    ///
    /// Returns `Ok(Some(lock))` if the lock was acquired, `Ok(None)` if
    /// another process holds it, or `Err` on I/O errors or missing user
    /// storage.
    pub fn try_lock_conversation(
        &self,
        conversation_id: &str,
        session: Option<&str>,
    ) -> Result<Option<ConversationFileLock>> {
        let user = self
            .user
            .as_deref()
            .ok_or(crate::Error::NotDir(Utf8PathBuf::from("<no user storage>")))?;

        let path = user.join(LOCKS_DIR).join(format!("{conversation_id}.lock"));

        ConversationFileLock::try_acquire(path, session)
    }

    /// Read lock holder info for a conversation.
    ///
    /// Returns `None` if there's no lock file, no user storage, or the
    /// file can't be parsed.
    #[must_use]
    pub fn read_conversation_lock_info(&self, conversation_id: &str) -> Option<LockInfo> {
        let user = self.user.as_deref()?;
        let path = user.join(LOCKS_DIR).join(format!("{conversation_id}.lock"));
        read_lock_info(&path)
    }
}

/// Check whether a lock file is orphaned (no process holds the lock).
///
/// Opens the file, attempts a non-blocking exclusive lock. If it succeeds,
/// the file is orphaned. The lock is immediately released.
pub(crate) fn is_orphaned_lock(path: &camino::Utf8Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
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

#[cfg(unix)]
fn try_exclusive_lock(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;
    // SAFETY: flock is a standard POSIX function. The file descriptor is valid
    // because we hold a reference to the open File.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(windows)]
fn try_exclusive_lock(file: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    // Lock the first byte of the file exclusively, non-blocking.
    let handle = file.as_raw_handle() as isize;
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    let flags = windows_sys::Win32::Storage::FileSystem::LOCKFILE_EXCLUSIVE_LOCK
        | windows_sys::Win32::Storage::FileSystem::LOCKFILE_FAIL_IMMEDIATELY;
    // SAFETY: handle is valid (from an open File), overlapped is zeroed.
    unsafe {
        windows_sys::Win32::Storage::FileSystem::LockFileEx(handle, flags, 0, 1, 0, &mut overlapped)
            != 0
    }
}

#[cfg(not(any(unix, windows)))]
fn try_exclusive_lock(_file: &File) -> bool {
    // No locking support; assume success (best-effort).
    true
}
