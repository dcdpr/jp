//! Working-tree and `base`-command file snapshots for a review.

use std::{
    collections::BTreeMap,
    fs, io,
    process::{Command, Stdio},
};

use camino::Utf8Path;
use chrono::{DateTime, Utc};

use crate::{
    model::Review,
    store::{self, Blobs},
};

/// Map from tracked file path to its UTF-8 contents, or `None` if the file is
/// absent from the working tree or base revision.
pub type Contents = BTreeMap<String, Option<String>>;

/// Snapshot the working tree in `root` for `review`'s tracked files, writing
/// new blobs and appending a new version if any file differs from the latest
/// version.
///
/// Returns the version number matching the current working tree.
pub fn record(
    root: &Utf8Path,
    review: &mut Review,
    blobs: &Blobs,
    at: DateTime<Utc>,
) -> Result<u32, String> {
    let contents = working_tree(root, &review.paths)?;
    let files = store::manifest(&contents);
    if let Some(latest) = review.latest().filter(|latest| latest.files == files) {
        return Ok(latest.number);
    }

    for text in contents.values().flatten() {
        blobs.put(text)?;
    }

    Ok(review.record(files, None, at))
}

/// Whether the working tree in `root` holds anything other than `review`'s
/// latest version of its tracked files, a file gone missing included; `true`
/// for a review with no versions.
pub fn changed(root: &Utf8Path, review: &Review) -> Result<bool, String> {
    let files = store::manifest(&working_tree(root, &review.paths)?);

    Ok(review.latest().is_none_or(|latest| latest.files != files))
}

/// Read each tracked path in `paths` (relative to `root`) from the working
/// tree.
///
/// Missing files map to `None`; non-UTF-8 files return an error.
pub fn working_tree(root: &Utf8Path, paths: &[String]) -> Result<Contents, String> {
    paths
        .iter()
        .map(|path| Ok((path.clone(), read(&root.join(path))?)))
        .collect()
}

fn read(path: &Utf8Path) -> Result<Option<String>, String> {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| format!("{path} is not UTF-8 text, which a review can't comment on.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{path}: {error}")),
    }
}

/// Run `command` in `root` for each path in `paths` (substituting `{path}` in
/// arguments) and return the captured UTF-8 stdout per file.
///
/// Files for which the command exits non-zero or emits non-UTF-8 bytes map to
/// `None`; returns an error if the program itself cannot be spawned.
pub fn base(root: &Utf8Path, command: &[String], paths: &[String]) -> Result<Contents, String> {
    let (program, args) = command
        .split_first()
        .ok_or("`plugins.command.review.options.base` names no program to run.")?;

    paths
        .iter()
        .map(|path| Ok((path.clone(), base_of(root, program, args, path)?)))
        .collect()
}

fn base_of(
    root: &Utf8Path,
    program: &str,
    args: &[String],
    path: &str,
) -> Result<Option<String>, String> {
    let output = Command::new(program)
        .args(args.iter().map(|arg| arg.replace("{path}", path)))
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("The `base` command, `{program}`, couldn't run: {error}"))?;

    if !output.status.success() {
        return Ok(None);
    }

    Ok(String::from_utf8(output.stdout).ok())
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
