//! The paths a review records, and reading the paths a user types as them.
//!
//! A review names each file relative to the workspace root, with `/` between
//! components, whichever directory a command runs in.
//! A path the user types is read relative to the directory `jp` ran in first,
//! then relative to the workspace root.

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

/// `typed`, made absolute against `base` and normalized lexically, as a path
/// relative to `root`: `docs/a.md`.
///
/// `None` when it lies outside `root`, or is `root` itself.
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

/// The file `typed` names, relative to `root`: read against `cwd`, then against
/// `root`.
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

/// The path among `tracked` that `typed` names: read against `cwd`, then
/// against `root`.
pub fn tracked(root: &Utf8Path, cwd: &Utf8Path, tracked: &[String], typed: &str) -> Option<String> {
    [cwd, root]
        .into_iter()
        .filter_map(|base| relative(root, base, typed))
        .find(|path| tracked.contains(path))
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
