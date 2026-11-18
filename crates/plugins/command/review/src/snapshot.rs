//! What a review's files hold: in the working tree, and as the `base` command
//! prints them.
//!
//! [`record`] keeps a review's latest version in step with the working tree.

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

/// What each file holds, `None` when there is nothing to record: the file is
/// absent from the working tree, or has no base.
pub type Contents = BTreeMap<String, Option<String>>;

/// Record what the working tree in `root` holds of `review`'s files as its next
/// version, unless its latest version holds the same, returning the number of
/// the version that holds it.
///
/// The contents go into `blobs` before the version that names them.
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

/// Read each of `paths`, relative to the workspace `root`, from the working
/// tree.
///
/// A file that isn't there is absent; one that isn't UTF-8 text is an error.
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

/// Run the `base` command for each of `paths` in the workspace `root`, and
/// collect what it prints.
///
/// `command` is the program and its arguments, `{path}` in an argument standing
/// for the file.
/// A file the command exits non-zero for, or prints something other than UTF-8
/// for, has no base; a program that can't be run is an error.
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
