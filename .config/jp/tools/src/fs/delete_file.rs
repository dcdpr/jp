use std::{fs, io};

use camino::Utf8Path;
use jp_tool::{Outcome, Question};
use serde_json::{Map, Value};

use super::utils::{is_file_dirty, resolve_workspace_entry};
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

    // Use `symlink_metadata` so a dangling symlink reads as "entry exists
    // and is a symlink" rather than "missing." `fs::remove_file` later
    // removes the link entry regardless of target health.
    let meta = match fs::symlink_metadata(&resolved.absolute) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return error("Path points to non-existing entry");
        }
        Err(e) => return Err(e.into()),
    };

    if meta.file_type().is_dir() {
        return error(
            "Path is a directory. You can only delete files. Empty directories are automatically \
             deleted.",
        );
    }

    let Some(parent) = resolved.absolute.parent() else {
        return error("Path has no parent");
    };

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

    if parent.read_dir()?.next().is_none() {
        fs::remove_dir(parent)?;
        msg.push_str(" Removed empty parent directory.");
    }

    Ok(msg.into())
}
