use std::fs;

use camino::Utf8Path;
use jp_tool::{AccessPolicy, Capability, Outcome, Question};
use serde_json::{Map, Value};

use super::utils::{
    EntryKind, ResolvedPath, authorize, authorize_entry, count_dirty_paths_impl,
    is_file_dirty_impl, resolve_workspace_entry,
};
use crate::{
    Error,
    util::{
        ToolResult, error,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// Kind of source entry being moved.
///
/// Symlinks are treated as `File`: `resolve_workspace_entry` leaves the final
/// component alone, so `fs::rename` renames the link entry itself rather than
/// its target.
/// The target is untouched and any other links to it stay intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    File,
    Dir,
}

pub(crate) async fn fs_move_file(
    root: &Utf8Path,
    access: Option<&AccessPolicy>,
    answers: &Map<String, Value>,
    source: String,
    target: String,
) -> ToolResult {
    fs_move_file_impl(root, access, answers, &source, &target, &DuctProcessRunner)
}

fn fs_move_file_impl<R: ProcessRunner>(
    root: &Utf8Path,
    access: Option<&AccessPolicy>,
    answers: &Map<String, Value>,
    source: &str,
    target: &str,
    runner: &R,
) -> ToolResult {
    let src = match resolve_workspace_entry(root, source, access) {
        Ok(r) => r,
        Err(msg) => return error(msg),
    };
    let dst = match resolve_workspace_entry(root, target, access) {
        Ok(r) => r,
        Err(msg) => return error(msg),
    };

    // The source entry is removed. A directory source takes its whole subtree
    // along, which `authorize` reads off the resolved entry.
    if let Err(msg) = authorize(access, Capability::Delete, &src) {
        return error(msg);
    }

    if src.absolute == dst.absolute {
        return error(format!(
            "Source and target resolve to the same path ('{source}')."
        ));
    }

    let src_kind = match classify_source(src.kind, source) {
        Some(Ok(kind)) => kind,
        Some(Err(message)) => return error(message),
        None => return error(format!("Source path '{source}' does not exist.")),
    };

    // For directory moves, the destination must not lie inside the source's
    // own subtree. `fs::rename` would fail with EINVAL, but only after
    // `create_dir_all` had already materialized intermediate parents under
    // `src` — leaving the workspace in a partial state after a failed move.
    // Catch this before any state-mutating I/O.
    if src_kind == SourceKind::Dir && dst.absolute.starts_with(&src.absolute) {
        return error(format!(
            "Cannot move directory '{source}' into a subdirectory of itself ('{target}')."
        ));
    }

    // Destination policy differs by source kind. For files we keep the
    // existing overwrite-with-confirmation behavior; for directories the
    // target must not exist at all (no implicit "move into" semantics).
    //
    // The kind comes from `symlink_metadata` rather than
    // `is_dir`/`is_file`/`exists`: those follow symlinks and lie about a
    // dangling final-position link, which would let `fs::rename` silently
    // replace the link without triggering the overwrite prompt and bypass
    // the directory "must not exist" rule.
    let dst_kind = dst.kind;

    // Overwriting an existing target needs `update`; a fresh target needs
    // `create`.
    let target_capability = if dst_kind.is_some() {
        Capability::Update
    } else {
        Capability::Create
    };

    // How far the write at the destination reaches is the *source's* shape, not
    // the destination's: moving a directory lands every path under it below
    // `dst`, whether or not anything sits there yet. `authorize` would read the
    // destination's own kind, which for a fresh target is no kind at all.
    let writes_a_subtree = src_kind == SourceKind::Dir;
    if let Err(msg) = authorize_entry(access, target_capability, &dst.relative, writes_a_subtree) {
        return error(msg);
    }

    match src_kind {
        SourceKind::File => match dst_kind {
            Some(EntryKind::Dir) => {
                return error(format!(
                    "Destination path '{target}' is an existing directory."
                ));
            }
            // File / Symlink / Other — the user named `dst` and `fs::rename`
            // will replace whichever entry sits there. Prompt before doing
            // so. Lumping symlinks in here is consistent with the source
            // side, where the link entry itself is what gets renamed.
            Some(_) => {
                if let Some(outcome) = confirm_overwrite_file(answers, target)? {
                    return Ok(outcome);
                }
            }
            None => {}
        },
        SourceKind::Dir => {
            if dst_kind.is_some() {
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

    // Clean up an empty parent directory of the source. Same logic for
    // both files and directories — once the entry is gone, the parent may
    // have nothing left. Gated on the *relative* parent being non-empty so
    // we never try to remove the workspace root itself when the source
    // lived at the top level.
    if let Some(parent) = empty_parent_to_remove(&src, access)? {
        fs::remove_dir(parent)?;
        msg.push_str(" Removed empty parent directory.");
    }

    Ok(msg.into())
}

/// Return the source's intermediate parent directory if it is now empty and
/// safe to remove.
///
/// Mirrors `delete_file::empty_parent_to_remove`: gated on the relative parent
/// being non-empty, so removing a top-level entry never tries to remove the
/// workspace root, and left in place without a grant of its own.
fn empty_parent_to_remove<'a>(
    resolved: &'a ResolvedPath,
    access: Option<&AccessPolicy>,
) -> Result<Option<&'a Utf8Path>, std::io::Error> {
    let Some(rel_parent) = resolved.relative.parent() else {
        return Ok(None);
    };
    if rel_parent.as_str().is_empty() {
        return Ok(None);
    }
    let Some(parent) = resolved.absolute.parent() else {
        return Ok(None);
    };
    // The parent may not exist anymore if the source was itself a
    // directory that has just been renamed out. `read_dir` would error in
    // that case; bail quietly instead.
    if !parent.exists() {
        return Ok(None);
    }
    if parent.read_dir()?.next().is_some() {
        return Ok(None);
    }

    // The same second deletion `delete_file` guards: a path the tool call never
    // named needs its own grant, and a refusal leaves the directory rather than
    // failing a move that already happened.
    if authorize_entry(access, Capability::Delete, rel_parent, true).is_err() {
        return Ok(None);
    }

    Ok(Some(parent))
}

/// Classify the resolved source entry for the move.
///
/// Returns `None` when the source does not exist, `Some(Ok(kind))` when it can
/// be moved, and `Some(Err(message))` for unsupported entry types (block
/// device, fifo, socket, ...).
fn classify_source(kind: Option<EntryKind>, source: &str) -> Option<Result<SourceKind, String>> {
    // `resolve_workspace_entry` leaves the final component alone, so the
    // observed entry kind is what the user named. Symlinks are bundled
    // with `File`: `fs::rename` will rename the link itself.
    Some(match kind? {
        EntryKind::Symlink | EntryKind::File => Ok(SourceKind::File),
        EntryKind::Dir => Ok(SourceKind::Dir),
        EntryKind::Other => Err(format!(
            "Source '{source}' is neither a regular file, symlink, nor a directory."
        )),
    })
}

fn confirm_overwrite_file(
    answers: &Map<String, Value>,
    target: &str,
) -> Result<Option<Outcome>, Error> {
    match answers.get("overwrite_file").and_then(Value::as_bool) {
        Some(true) => Ok(None),
        Some(false) => Ok(Some(error_outcome(format!(
            "Destination '{target}' already exists."
        )))),
        None => {
            let question = Question::boolean(
                "overwrite_file",
                format!("Destination '{target}' exists. Overwrite?"),
            )?
            .with_default(Value::Bool(false));
            Ok(Some(Outcome::NeedsInput { question }))
        }
    }
}

/// Build a transient error outcome from a plain message.
/// Mirrors `crate::util::error` but returns the bare `Outcome` so callers that
/// need to embed it in another `Result` shape don't have to unwrap.
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
        None => {
            let question =
                Question::boolean("move_dirty_source", prompt)?.with_default(Value::Bool(false));
            Ok(Some(Outcome::NeedsInput { question }))
        }
    }
}

#[cfg(test)]
#[path = "move_file_tests.rs"]
mod tests;
