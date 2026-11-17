//! On-disk cache for dashboard sources.
//!
//! Each source keeps one file per repository under `$XDG_CACHE_HOME/jp/dash/`
//! (`~/.cache/jp/dash/` when unset), named after the common git directory
//! shared by every worktree of the repository.
//! A missing, unreadable, or invalid cache file deserializes to the default
//! value.

use std::{env, fs, io, process};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Serialize, de::DeserializeOwned};

use super::git;

/// Return the cache file path for source `kind` and the repository containing
/// `dir`.
///
/// Returns `None` if the XDG cache directory or the repository's common git
/// directory cannot be determined.
pub fn file(dir: &Utf8Path, kind: &str) -> Option<Utf8PathBuf> {
    let home = home(env::var("XDG_CACHE_HOME").ok(), env::var("HOME").ok())?;
    let repository = git::common_dir(dir).ok()?;
    Some(path(&home, &repository, kind))
}

/// Deserialize `T` from `file`, falling back to `T::default()` if the file is
/// missing, unreadable, or invalid JSON.
pub fn load<T: DeserializeOwned + Default>(file: &Utf8Path) -> T {
    fs::read_to_string(file)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Serialize `value` as JSON and write it atomically to `file`.
pub fn store<T: Serialize>(file: &Utf8Path, value: &T) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(value).map_err(io::Error::other)?;

    // Write to a temporary file and rename it into place so a concurrent
    // `dash` process never reads a partial file.
    let partial = file.with_extension(format!("{}.tmp", process::id()));
    fs::write(&partial, json)?;
    fs::rename(&partial, file)
}

/// Return `$XDG_CACHE_HOME` if set to an absolute path, falling back to
/// `$HOME/.cache`.
fn home(xdg: Option<String>, home: Option<String>) -> Option<Utf8PathBuf> {
    xdg.map(Utf8PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            let home = home.filter(|home| !home.is_empty())?;
            Some(Utf8PathBuf::from(home).join(".cache"))
        })
}

/// Build the cache file path under `home` for source `kind` and git directory
/// `repository`.
fn path(home: &Utf8Path, repository: &Utf8Path, kind: &str) -> Utf8PathBuf {
    let name: String = repository
        .as_str()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    home.join("jp")
        .join("dash")
        .join(format!("{}.{kind}.json", name.trim_matches('-')))
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
