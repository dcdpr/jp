//! Cross-process resource locking.
//!
//! [`ResourceLocker`] serializes access to named resources.
//! It is the locking *primitive*: acquire, release on drop, probe, and opaque
//! holder info.
//! Domain policies — which resource names exist, what the holder info
//! contains, where lock files live — belong to the consumer.
//!
//! Three implementations:
//!
//! - [`FsResourceLocker`] — one `<resource>.lock` file per resource, held via
//!   OS advisory locks (`flock` on Unix, `LockFileEx` on Windows).
//! - [`InMemoryResourceLocker`] — in-process locking for tests and
//!   memory-backed setups.
//! - [`NullResourceLocker`] — every acquisition succeeds; for flows that
//!   deliberately run without cross-process exclusion.

use std::{
    collections::HashMap,
    fmt::Debug,
    fs::{File, OpenOptions},
    io::{self, Read as _, Seek as _, Write as _},
    sync::{Arc, Condvar, Mutex},
};

use camino::Utf8PathBuf;

/// Failure to acquire or inspect a resource lock.
#[derive(Debug, thiserror::Error)]
#[error("failed to lock resource {resource:?}: {source}")]
pub struct LockError {
    /// The resource whose lock operation failed.
    pub resource: String,

    #[source]
    pub source: io::Error,
}

impl LockError {
    fn new(resource: &str, source: io::Error) -> Self {
        Self {
            resource: resource.to_owned(),
            source,
        }
    }
}

/// Serializes access to named resources across processes.
pub trait ResourceLocker: Send + Sync + Debug {
    /// Attempt to acquire the resource's exclusive lock without blocking.
    ///
    /// Returns `Ok(None)` when another holder has it.
    /// `info` is opaque holder metadata recorded best-effort at acquisition,
    /// readable through [`Self::holder_info`] while the lock file (or in-memory
    /// entry) exists.
    /// `None` records nothing, and clears anything an earlier holder left.
    fn try_lock(
        &self,
        resource: &str,
        info: Option<&str>,
    ) -> Result<Option<Box<dyn ResourceGuard>>, LockError>;

    /// Block until the resource's exclusive lock is acquired.
    fn lock(&self, resource: &str, info: Option<&str>)
    -> Result<Box<dyn ResourceGuard>, LockError>;

    /// Whether another holder currently has the resource.
    ///
    /// A non-destructive probe: no lock file is created or removed.
    fn is_held(&self, resource: &str) -> bool;

    /// Read the info recorded by the most recent acquisition (best-effort).
    ///
    /// Not proof that anyone holds the resource: a file-based locker that keeps
    /// its lock files reports the last holder's info after that holder's guard
    /// dropped.
    /// Pair this with [`Self::is_held`] when the current holder is what
    /// matters.
    fn holder_info(&self, resource: &str) -> Option<String>;
}

/// A held resource lock.
/// Released on drop.
pub trait ResourceGuard: Send + Sync + Debug {}

/// File-based locking: one `<resource>.lock` file per resource under a fixed
/// directory, held via OS advisory locks.
///
/// Killing the holding process releases the OS lock automatically; the lock
/// file may remain, but a leftover file does not block the next acquisition.
#[derive(Debug, Clone)]
pub struct FsResourceLocker {
    dir: Utf8PathBuf,

    /// Remove the lock file when the guard drops.
    ///
    /// Unlinking is never fully sound.
    /// The OS lock releases when the handle closes, so a contender that opens
    /// the path in the window before the unlink takes a lock on an inode about
    /// to disappear, while the next acquisition creates and locks a fresh file
    /// at the same path: two holders at once.
    /// RFD 106 calls this the release-then-unlink defect.
    ///
    /// [`ResourceLocker::try_lock`] bounds that window to the few instructions
    /// between the close and the unlink.
    /// [`ResourceLocker::lock`] would make it a certainty, because a blocking
    /// waiter is already parked on the doomed inode when the unlink lands, and
    /// is refused for that reason.
    remove_on_drop: bool,
}

impl FsResourceLocker {
    /// A locker placing its lock files in `dir` (created on demand).
    pub fn new(dir: impl Into<Utf8PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            remove_on_drop: false,
        }
    }

    /// Remove lock files when their guard drops.
    ///
    /// Restricts the locker to [`ResourceLocker::try_lock`]:
    /// [`ResourceLocker::lock`] returns an error, because a blocking waiter
    /// parks on the inode a concurrent drop is about to unlink and would end up
    /// holding it alongside whoever locks the recreated path.
    #[must_use]
    pub fn with_remove_on_drop(mut self) -> Self {
        self.remove_on_drop = true;
        self
    }

    fn lock_path(&self, resource: &str) -> Utf8PathBuf {
        self.dir.join(format!("{resource}.lock"))
    }

    /// Open (or create) the lock file for `resource`.
    fn open(&self, resource: &str) -> Result<(File, Utf8PathBuf), LockError> {
        let path = self.lock_path(resource);
        std::fs::create_dir_all(&self.dir).map_err(|e| LockError::new(resource, e))?;

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&path)
            .map_err(|e| LockError::new(resource, e))?;

        Ok((file, path))
    }

    fn guard(&self, file: File, path: Utf8PathBuf, info: Option<&str>) -> Box<dyn ResourceGuard> {
        let mut guard = FsResourceGuard {
            file: Some(file),
            path,
            remove_on_drop: self.remove_on_drop,
        };

        // Holder info is purely diagnostic; failing to record it must not fail
        // the acquisition. `None` truncates, so a file left behind by an
        // earlier holder never reports that holder's info as this one's.
        let _err = guard.write_info(info.unwrap_or(""));

        Box::new(guard)
    }
}

impl ResourceLocker for FsResourceLocker {
    fn try_lock(
        &self,
        resource: &str,
        info: Option<&str>,
    ) -> Result<Option<Box<dyn ResourceGuard>>, LockError> {
        let (file, path) = self.open(resource)?;

        if !try_exclusive_lock(&file) {
            return Ok(None);
        }

        Ok(Some(self.guard(file, path, info)))
    }

    fn lock(
        &self,
        resource: &str,
        info: Option<&str>,
    ) -> Result<Box<dyn ResourceGuard>, LockError> {
        // Refused before the file is created: a blocking waiter parks on the
        // inode a concurrent drop is about to unlink, and would hold it
        // alongside whoever locks the recreated path.
        if self.remove_on_drop {
            return Err(LockError::new(
                resource,
                io::Error::other("blocking lock is unavailable with remove_on_drop"),
            ));
        }

        let (file, path) = self.open(resource)?;

        if !exclusive_lock(&file) {
            return Err(LockError::new(resource, io::Error::last_os_error()));
        }

        Ok(self.guard(file, path, info))
    }

    fn is_held(&self, resource: &str) -> bool {
        let Ok(file) = File::open(self.lock_path(resource)) else {
            return false;
        };

        // Acquiring the probe lock proves nobody holds it; the probe's own
        // lock releases when `file` drops, and the file is left in place.
        !try_exclusive_lock(&file)
    }

    fn holder_info(&self, resource: &str) -> Option<String> {
        let mut file = File::open(self.lock_path(resource)).ok()?;
        let mut buf = String::new();
        file.read_to_string(&mut buf).ok()?;

        (!buf.is_empty()).then_some(buf)
    }
}

/// A held file lock.
///
/// The OS lock is held as long as the `File` is open.
#[derive(Debug)]
struct FsResourceGuard {
    file: Option<File>,
    path: Utf8PathBuf,
    remove_on_drop: bool,
}

impl FsResourceGuard {
    /// Record holder info in the lock file (best-effort).
    fn write_info(&mut self, info: &str) -> io::Result<()> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };

        file.set_len(0)?;
        file.seek(io::SeekFrom::Start(0))?;
        file.write_all(info.as_bytes())?;
        file.flush()
    }
}

impl Drop for FsResourceGuard {
    fn drop(&mut self) {
        // Drop the file handle first to release the OS lock.
        self.file.take();

        if self.remove_on_drop {
            let _err = std::fs::remove_file(&self.path);
        }
    }
}

impl ResourceGuard for FsResourceGuard {}

/// In-process locking backed by a mutex-guarded map.
///
/// Blocking acquisitions wait on a condition variable notified by guard drops.
#[derive(Debug, Clone, Default)]
pub struct InMemoryResourceLocker {
    state: Arc<LockState>,
}

#[derive(Debug, Default)]
struct LockState {
    held: Mutex<HashMap<String, Option<String>>>,
    released: Condvar,
}

impl InMemoryResourceLocker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn guard(&self, resource: &str) -> Box<dyn ResourceGuard> {
        Box::new(InMemoryResourceGuard {
            resource: resource.to_owned(),
            state: Arc::clone(&self.state),
        })
    }
}

impl ResourceLocker for InMemoryResourceLocker {
    fn try_lock(
        &self,
        resource: &str,
        info: Option<&str>,
    ) -> Result<Option<Box<dyn ResourceGuard>>, LockError> {
        let mut held = self.state.held.lock().expect("poisoned");
        if held.contains_key(resource) {
            return Ok(None);
        }

        held.insert(resource.to_owned(), info.map(str::to_owned));
        drop(held);

        Ok(Some(self.guard(resource)))
    }

    fn lock(
        &self,
        resource: &str,
        info: Option<&str>,
    ) -> Result<Box<dyn ResourceGuard>, LockError> {
        let mut held = self.state.held.lock().expect("poisoned");
        while held.contains_key(resource) {
            held = self.state.released.wait(held).expect("poisoned");
        }

        held.insert(resource.to_owned(), info.map(str::to_owned));
        drop(held);

        Ok(self.guard(resource))
    }

    fn is_held(&self, resource: &str) -> bool {
        self.state
            .held
            .lock()
            .expect("poisoned")
            .contains_key(resource)
    }

    fn holder_info(&self, resource: &str) -> Option<String> {
        self.state
            .held
            .lock()
            .expect("poisoned")
            .get(resource)
            .cloned()
            .flatten()
    }
}

/// A held in-process lock.
/// Removes its entry and wakes blocked waiters on drop.
#[derive(Debug)]
struct InMemoryResourceGuard {
    resource: String,
    state: Arc<LockState>,
}

impl Drop for InMemoryResourceGuard {
    fn drop(&mut self) {
        self.state
            .held
            .lock()
            .expect("poisoned")
            .remove(&self.resource);
        self.state.released.notify_all();
    }
}

impl ResourceGuard for InMemoryResourceGuard {}

/// No-op locking: every acquisition succeeds, nothing is ever held.
///
/// For flows that deliberately run without cross-process exclusion (e.g.
/// ephemeral no-persist runs).
/// Never use it where unserialized mutation can corrupt durable state.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullResourceLocker;

#[derive(Debug)]
struct NullResourceGuard;

impl ResourceGuard for NullResourceGuard {}

impl ResourceLocker for NullResourceLocker {
    fn try_lock(
        &self,
        _resource: &str,
        _info: Option<&str>,
    ) -> Result<Option<Box<dyn ResourceGuard>>, LockError> {
        Ok(Some(Box::new(NullResourceGuard)))
    }

    fn lock(
        &self,
        _resource: &str,
        _info: Option<&str>,
    ) -> Result<Box<dyn ResourceGuard>, LockError> {
        Ok(Box::new(NullResourceGuard))
    }

    fn is_held(&self, _resource: &str) -> bool {
        false
    }

    fn holder_info(&self, _resource: &str) -> Option<String> {
        None
    }
}

#[cfg(unix)]
pub(crate) fn try_exclusive_lock(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;

    // SAFETY: flock is a standard POSIX function. The file descriptor is valid
    // because we hold a reference to the open File.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

#[cfg(unix)]
fn exclusive_lock(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;

    // SAFETY: see `try_exclusive_lock`.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) == 0 }
}

#[cfg(windows)]
pub(crate) fn try_exclusive_lock(file: &File) -> bool {
    windows_lock(file, true)
}

#[cfg(windows)]
fn exclusive_lock(file: &File) -> bool {
    windows_lock(file, false)
}

#[cfg(windows)]
fn windows_lock(file: &File, fail_immediately: bool) -> bool {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::{
        Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx},
        System::IO::OVERLAPPED,
    };

    let handle = file.as_raw_handle();
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let mut flags = LOCKFILE_EXCLUSIVE_LOCK;
    if fail_immediately {
        flags |= LOCKFILE_FAIL_IMMEDIATELY;
    }

    // Lock a single byte at a high offset, far past any holder info written
    // at offset 0. Windows exclusive byte-range locks prevent ALL other
    // handles from reading the locked region, so placing the lock away from
    // the file content lets other handles read the holder info.
    overlapped.Anonymous.Anonymous.Offset = u32::MAX;

    // SAFETY: handle is valid (from an open File), overlapped is initialized.
    unsafe { LockFileEx(handle, flags, 0, 1, 0, &mut overlapped) != 0 }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn try_exclusive_lock(_file: &File) -> bool {
    // No locking support; assume success (best-effort).
    true
}

#[cfg(not(any(unix, windows)))]
fn exclusive_lock(_file: &File) -> bool {
    true
}

#[cfg(test)]
#[path = "resource_lock_tests.rs"]
mod tests;
