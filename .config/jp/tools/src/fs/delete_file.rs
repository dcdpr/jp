use std::fs;

use camino::Utf8Path;
use jp_tool::{AccessPolicy, Capability, Outcome, Question};
use serde_json::{Map, Value};

use super::utils::{
    EntryKind, ResolvedPath, authorize, authorize_entry, is_file_dirty, resolve_workspace_entry,
};
use crate::util::{ToolResult, error};

pub(crate) async fn fs_delete_file(
    root: &Utf8Path,
    access: Option<&AccessPolicy>,
    answers: &Map<String, Value>,
    path: String,
) -> ToolResult {
    let resolved = match resolve_workspace_entry(root, &path, access) {
        Ok(r) => r,
        Err(msg) => return error(msg),
    };

    if let Err(msg) = authorize(access, Capability::Delete, &resolved) {
        return error(msg);
    }

    match resolved.kind {
        None => return error("Path points to non-existing entry"),
        Some(EntryKind::Dir) => {
            return error(
                "Path is a directory. You can only delete files. Empty directories are \
                 automatically deleted.",
            );
        }
        // Refuse on unusual entry types for consistency with `create_file`
        // and `move_file::classify_source`. `fs::remove_file` would happily
        // unlink a socket/FIFO/device, but the user-facing tool is "delete
        // a file" — surfacing the kind lets the user reach for a different
        // tool if they actually meant it.
        Some(EntryKind::Other) => {
            return error("Path is not a regular file, symlink, or directory.");
        }
        // File and Symlink (live or dangling) — both removable via
        // `fs::remove_file`, which unlinks the entry without following.
        Some(EntryKind::File | EntryKind::Symlink) => {}
    }

    if is_file_dirty(root, &resolved.relative)? {
        match answers.get("delete_dirty_file").and_then(Value::as_bool) {
            Some(true) => {}
            Some(false) => {
                return error("File has uncommitted changes. Please stage or discard first.");
            }
            None => {
                let question = Question::boolean(
                    "delete_dirty_file",
                    format!("File '{path}' has uncommitted changes. Delete anyway?"),
                )?
                .with_default(Value::Bool(false));
                return Ok(Outcome::NeedsInput { question });
            }
        }
    }

    fs::remove_file(&resolved.absolute)?;
    let mut msg = "File deleted.".to_owned();

    if let Some(parent) = empty_parent_to_remove(&resolved, access)? {
        fs::remove_dir(parent)?;
        msg.push_str(" Removed empty parent directory.");
    }

    Ok(msg.into())
}

/// Return the entry's intermediate parent directory if it is now empty and safe
/// to remove.
///
/// "Intermediate" means: not the workspace root itself.
/// The check is gated on the *relative* parent being non-empty, which is true
/// exactly when the deleted entry lived in a subdirectory.
/// This protects against deleting the workspace itself when the entry was at
/// the top level — in that case `resolved.absolute.parent()` is the canonical
/// workspace root, and removing it would either error (CWD/EBUSY) or, worse,
/// succeed.
///
/// The parent needs its own grant, and is left in place without one.
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
    if parent.read_dir()?.next().is_some() {
        return Ok(None);
    }

    // Removing the directory is a second deletion, of a path the tool call
    // never named, so it is authorized separately: a policy may hand out
    // `delete` on the files in a tree and still refuse the tree itself. The
    // parent is a directory, hence the subtree question — which an empty
    // directory only fails when a rule below it denies.
    //
    // A refusal skips the tidy-up rather than failing the call: the file the
    // caller asked about is already gone, and reporting an error for the
    // leftover directory would read as if the delete had not happened.
    if authorize_entry(access, Capability::Delete, rel_parent, true).is_err() {
        return Ok(None);
    }

    Ok(Some(parent))
}

#[cfg(test)]
#[path = "delete_file_tests.rs"]
mod tests;
