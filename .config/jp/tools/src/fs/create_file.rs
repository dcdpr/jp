use std::{
    fs::{self, File},
    io::Write as _,
};

use crossterm::style::Stylize;
use jp_md::format::Formatter;
use jp_tool::{Outcome, Question};
use serde_json::{Map, Value};

use super::utils::resolve_workspace_entry;
use crate::{
    Context,
    util::{ToolResult, error, fail},
};

pub(crate) async fn fs_create_file(
    ctx: Context,
    answers: &Map<String, Value>,
    path: String,
    content: Option<String>,
) -> ToolResult {
    let resolved = match resolve_workspace_entry(&ctx.root, &path) {
        Ok(r) => r,
        Err(msg) => return error(msg),
    };

    if ctx.action.is_format_arguments() {
        let lang = crate::util::lang_from_path(&path);

        let mut response = format!("Creating file '{}'", path.as_str().bold().blue());
        if let Some(content) = content {
            let code_block = format!("`````{lang}\n{content}\n`````");
            let highlighted = Formatter::new()
                .format_terminal(&code_block)
                .unwrap_or(code_block);
            let header = response.clone();
            response.push_str(&format!(" with content:\n\n{highlighted}\n\n{header}"));
        }

        return Ok(response.into());
    }

    let absolute_path = resolved.absolute;
    if absolute_path.is_dir() {
        return error("Path is an existing directory.");
    }

    // Refuse to write through a symlink. `resolve_workspace_entry` left the
    // final component intact, so an existing symlink shows up here as a
    // symlink in `symlink_metadata`. `File::open(O_CREAT)` would follow it
    // and create whatever the link points at — silently if the target lies
    // outside the workspace. Users who really want to replace a link can
    // delete it first.
    if absolute_path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return error("Path is an existing symlink. Delete it first.");
    }

    if absolute_path.exists() {
        match answers.get("overwrite_file").and_then(Value::as_bool) {
            Some(true) => {}
            Some(false) => {
                return error("Path points to existing file");
            }
            None => {
                return Ok(Outcome::NeedsInput {
                    question: Question::boolean(
                        "overwrite_file",
                        format!("File '{path}' exists. Overwrite?"),
                    )
                    .with_default(Value::Bool(false)),
                });
            }
        }
    }

    let Some(parent) = absolute_path.parent() else {
        return fail("Path has no parent");
    };

    fs::create_dir_all(parent)?;
    let mut file = File::options()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&absolute_path)?;

    if let Some(content) = content {
        file.write_all(content.as_bytes())?;
    }

    Ok(format!(
        "File '{}' created. File size: {}",
        path,
        file.metadata()?.len()
    )
    .into())
}

#[cfg(test)]
#[path = "create_file_tests.rs"]
mod tests;
