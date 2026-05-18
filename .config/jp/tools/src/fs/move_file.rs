use std::fs;

use camino::Utf8Path;
use jp_tool::{Outcome, Question};
use serde_json::{Map, Value};

use super::utils::{count_dirty_paths_impl, is_file_dirty_impl, resolve_workspace_path};
use crate::{
    Error,
    util::{
        ToolResult, error,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// Kind of source entry being moved.
///
/// Note on symlinks: `resolve_workspace_path` canonicalizes through symlinks,
/// so a symlink-as-source resolves to its target before the rename. Moving a
/// symlink therefore moves the underlying file (and breaks the link), rather
/// than renaming the link itself. Symlinks pointing outside the workspace are
/// rejected by the resolver. Callers that need link-preserving semantics
/// should add a separate resolver primitive rather than special-case the move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    File,
    Dir,
}

pub(crate) async fn fs_move_file(
    root: &Utf8Path,
    answers: &Map<String, Value>,
    source: String,
    target: String,
) -> ToolResult {
    fs_move_file_impl(root, answers, &source, &target, &DuctProcessRunner)
}

fn fs_move_file_impl<R: ProcessRunner>(
    root: &Utf8Path,
    answers: &Map<String, Value>,
    source: &str,
    target: &str,
    runner: &R,
) -> ToolResult {
    let src = match resolve_workspace_path(root, source) {
        Ok(r) => r,
        Err(msg) => return error(msg),
    };
    let dst = match resolve_workspace_path(root, target) {
        Ok(r) => r,
        Err(msg) => return error(msg),
    };

    if src.absolute == dst.absolute {
        return error(format!(
            "Source and target resolve to the same path ('{source}')."
        ));
    }

    let src_kind = match classify_source(&src.absolute, source)? {
        Some(Ok(kind)) => kind,
        Some(Err(message)) => return error(message),
        None => return error(format!("Source path '{source}' does not exist.")),
    };

    // Destination policy differs by source kind. For files we keep the
    // existing overwrite-with-confirmation behavior; for directories the
    // target must not exist at all (no implicit "move into" semantics).
    match src_kind {
        SourceKind::File => {
            if dst.absolute.is_dir() {
                return error(format!(
                    "Destination path '{target}' is an existing directory."
                ));
            }
            if dst.absolute.is_file()
                && let Some(outcome) = confirm_overwrite_file(answers, target)
            {
                return Ok(outcome);
            }
        }
        SourceKind::Dir => {
            if dst.absolute.exists() {
                return error(format!(
                    "Destination path '{target}' already exists. The target path must not exist \
                     when moving a directory."
                ));
            }
        }
    }

    // Dirty check. Files use the existing single-entry rule (unstaged
    // modification only); directories use the broader "any porcelain output"
    // rule that captures every kind of uncommitted state under the tree.
    if let Some(outcome) =
        confirm_dirty_source(root, &src.relative, src_kind, source, runner, answers)?
    {
        return Ok(outcome);
    }

    if let Some(parent) = dst.absolute.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::rename(&src.absolute, &dst.absolute)?;
    let mut msg = match src_kind {
        SourceKind::File => format!("Moved file '{source}' to '{target}'."),
        SourceKind::Dir => format!("Moved directory '{source}' to '{target}'."),
    };

    // Clean up an empty parent directory of the source. Same logic for both
    // files and directories — once the entry is gone, the parent may have
    // nothing left.
    if let Some(parent) = src.absolute.parent()
        && parent != root
        && parent.exists()
        && parent.read_dir()?.next().is_none()
    {
        fs::remove_dir(parent)?;
        msg.push_str(" Removed empty parent directory.");
    }

    Ok(msg.into())
}

/// Classify the source entry by inspecting its file type.
///
/// Returns `Ok(None)` when the source does not exist, `Ok(Some(Ok(kind)))`
/// when it can be moved, and `Ok(Some(Err(message)))` for unsupported entry
/// types (block device, fifo, socket, ...). The outer `Result` propagates I/O
/// errors that aren't `NotFound`.
fn classify_source(
    absolute: &Utf8Path,
    source: &str,
) -> Result<Option<Result<SourceKind, String>>, Error> {
    let meta = match fs::symlink_metadata(absolute) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    // After resolving, `symlink_metadata` reports the canonical target's type,
    // not the original link's. A symlink would only show up here if the
    // resolver returned an absolute path that itself is still a link, which is
    // possible only for dangling links (no canonical target to follow). We
    // accept those as files.
    let ft = meta.file_type();
    let kind = if ft.is_symlink() || ft.is_file() {
        Ok(SourceKind::File)
    } else if ft.is_dir() {
        Ok(SourceKind::Dir)
    } else {
        Err(format!(
            "Source '{source}' is neither a regular file, symlink, nor a directory."
        ))
    };

    Ok(Some(kind))
}

fn confirm_overwrite_file(answers: &Map<String, Value>, target: &str) -> Option<Outcome> {
    match answers.get("overwrite_file").and_then(Value::as_bool) {
        Some(true) => None,
        Some(false) => Some(error_outcome(format!(
            "Destination '{target}' already exists."
        ))),
        None => Some(Outcome::NeedsInput {
            question: Question::boolean(
                "overwrite_file",
                format!("Destination '{target}' exists. Overwrite?"),
            )
            .with_default(Value::Bool(false)),
        }),
    }
}

/// Build a transient error outcome from a plain message. Mirrors
/// `crate::util::error` but returns the bare `Outcome` so callers that need to
/// embed it in another `Result` shape don't have to unwrap.
fn error_outcome(message: impl Into<String>) -> Outcome {
    Outcome::Error {
        message: message.into(),
        trace: vec![],
        transient: true,
    }
}

fn confirm_dirty_source<R: ProcessRunner>(
    root: &Utf8Path,
    relative: &Utf8Path,
    kind: SourceKind,
    source: &str,
    runner: &R,
    answers: &Map<String, Value>,
) -> Result<Option<Outcome>, Error> {
    let prompt = match kind {
        SourceKind::File => {
            if !is_file_dirty_impl(root, relative, runner)? {
                return Ok(None);
            }
            format!("File '{source}' has uncommitted changes. Move anyway?")
        }
        SourceKind::Dir => {
            let count = count_dirty_paths_impl(root, relative, runner)?;
            if count == 0 {
                return Ok(None);
            }
            let noun = if count == 1 { "entry" } else { "entries" };
            format!("Directory '{source}' contains {count} uncommitted {noun}. Move anyway?")
        }
    };

    match answers.get("move_dirty_source").and_then(Value::as_bool) {
        Some(true) => Ok(None),
        Some(false) => Ok(Some(error_outcome(format!(
            "'{source}' has uncommitted changes; please stage or discard first."
        )))),
        None => Ok(Some(Outcome::NeedsInput {
            question: Question::boolean("move_dirty_source", prompt)
                .with_default(Value::Bool(false)),
        })),
    }
}

#[cfg(test)]
#[path = "move_file_tests.rs"]
mod tests;
