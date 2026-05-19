use std::fs;

use camino::Utf8Path;
use jp_tool::{Outcome, Question};
use serde_json::{Map, Value};

use super::utils::{EntryKind, ResolvedPath, entry_kind, is_file_dirty, resolve_workspace_entry};
use crate::util::{ToolResult, error};

pub(crate) async fn fs_delete_file(
    root: &Utf8Path,
    answers: &Map<String, Value>,
    path: String,
) -> ToolResult {
    let resolved = match resolve_workspace_entry(root, &path) {
        Ok(r) => r,
        Err(msg) => return error(msg),
    };

    match entry_kind(&resolved.absolute)? {
        None => return error("Path points to non-existing entry"),
        Some(EntryKind::Dir) => {
            return error(
                "Path is a directory. You can only delete files. Empty directories are \
                 automatically deleted.",
            );
        }
        // File, Symlink (live or dangling), Other — all removable via
        // `fs::remove_file`, which unlinks the entry without following.
        Some(_) => {}
    }

    if is_file_dirty(root, &resolved.relative)? {
        match answers.get("delete_dirty_file").and_then(Value::as_bool) {
            Some(true) => {}
            Some(false) => {
                return error("File has uncommitted changes. Please stage or discard first.");
            }
            None => {
                return Ok(Outcome::NeedsInput {
                    question: Question::boolean(
                        "delete_dirty_file",
                        format!("File '{path}' has uncommitted changes. Delete anyway?"),
                    )
                    .with_default(Value::Bool(false)),
                });
            }
        }
    }

    fs::remove_file(&resolved.absolute)?;
    let mut msg = "File deleted.".to_owned();

    if let Some(parent) = empty_parent_to_remove(&resolved)? {
        fs::remove_dir(parent)?;
        msg.push_str(" Removed empty parent directory.");
    }

    Ok(msg.into())
}

/// Return the entry's intermediate parent directory if it is now empty and
/// safe to remove.
///
/// "Intermediate" means: not the workspace root itself. The check is gated
/// on the *relative* parent being non-empty, which is true exactly when the
/// deleted entry lived in a subdirectory. This protects against deleting
/// the workspace itself when the entry was at the top level — in that case
/// `resolved.absolute.parent()` is the canonical workspace root, and
/// removing it would either error (CWD/EBUSY) or, worse, succeed.
fn empty_parent_to_remove(resolved: &ResolvedPath) -> Result<Option<&Utf8Path>, std::io::Error> {
    let Some(rel_parent) = resolved.relative.parent() else {
        return Ok(None);
    };
    if rel_parent.as_str().is_empty() {
        return Ok(None);
    }
    let Some(parent) = resolved.absolute.parent() else {
        return Ok(None);
    };
    if parent.read_dir()?.next().is_some() {
        return Ok(None);
    }
    Ok(Some(parent))
}

#[cfg(test)]
#[path = "delete_file_tests.rs"]
mod tests;
