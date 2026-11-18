//! Path normalization and resolution for tracked review files.
//!
//! Tracked files are stored as `/`-separated paths relative to the workspace
//! root regardless of the working directory.
//! User-supplied paths are resolved relative to the current working directory
//! first, then relative to the workspace root.

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

/// Resolve `typed` against `base`, normalize `.` and `..` components lexically,
/// and return the `/`-separated path relative to `root` (e.g. `"docs/a.md"`).
///
/// Returns `None` if the resolved path is `root` itself or lies outside `root`.
pub fn relative(root: &Utf8Path, base: &Utf8Path, typed: &str) -> Option<String> {
    let mut path = Utf8PathBuf::new();
    for component in base.join(typed).components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                if !path.pop() {
                    return None;
                }
            }
            other => path.push(other),
        }
    }

    let parts: Vec<&str> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|component| component.as_str())
        .collect();

    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Resolve `typed` to an existing file relative to `root`, checking `cwd` first
/// and then `root`.
pub fn existing(root: &Utf8Path, cwd: &Utf8Path, typed: &str) -> Result<String, String> {
    let candidates: Vec<String> = [cwd, root]
        .into_iter()
        .filter_map(|base| relative(root, base, typed))
        .collect();

    if let Some(found) = candidates.iter().find(|path| root.join(path).is_file()) {
        return Ok(found.clone());
    }

    Err(if candidates.is_empty() {
        format!("{typed} is outside the workspace.")
    } else {
        format!("No file at {typed}.")
    })
}

/// Resolve `typed` to a path in `tracked`, checking `cwd` first and then
/// `root`.
pub fn tracked(root: &Utf8Path, cwd: &Utf8Path, tracked: &[String], typed: &str) -> Option<String> {
    [cwd, root]
        .into_iter()
        .filter_map(|base| relative(root, base, typed))
        .find(|path| tracked.contains(path))
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
