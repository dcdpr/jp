//! Exclusive advisory locks that serialize writers across processes.
//!
//! A lock is an OS advisory lock (`File::lock`) on a lock file that is created
//! on first use and never deleted: deleting it on release would let a new
//! writer lock a newly created inode while a waiting writer locks the unlinked
//! one.
//! The kernel releases a lock when its file is closed, including when the
//! process holding it exits or crashes.
//!
//! Advisory locks bind only the processes that take them, and some network
//! filesystems do not honor them.

use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
};

use camino::Utf8Path;

/// Acquire an exclusive lock on `path`, held until the returned [`File`] is
/// dropped.
///
/// Blocks while another process or open file descriptor holds the lock.
/// Creates the file if it does not exist, and returns `Ok(None)` if the parent
/// directory does not exist.
///
/// # Errors
///
/// Returns an error if the lock file cannot be opened or locked.
pub fn exclusive(path: &Utf8Path) -> io::Result<Option<File>> {
    let file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    file.lock()?;

    Ok(Some(file))
}

/// Try to acquire an exclusive lock on `path` without blocking, held until the
/// returned [`File`] is dropped.
///
/// Returns `Ok(None)` if another process or open file descriptor holds the
/// lock.
/// Creates the file if it does not exist.
///
/// # Errors
///
/// Returns an error if the lock file cannot be opened or locked, including when
/// its parent directory does not exist.
pub fn try_exclusive(path: &Utf8Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;

    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
