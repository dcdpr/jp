use jp_tool::{Capability, Context};

use super::utils::{authorize, resolve_workspace_path};
use crate::util::{ToolResult, error};

pub(crate) async fn fs_read_file(
    ctx: &Context,
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> ToolResult {
    let resolved = match resolve_workspace_path(&ctx.root, &path, ctx.access.as_ref()) {
        Ok(r) => r,
        Err(msg) => return error(msg),
    };
    if let Err(msg) = authorize(ctx.access.as_ref(), Capability::Read, &resolved.relative) {
        return error(msg);
    }
    let absolute_path = resolved.absolute;
    if !absolute_path.exists() {
        return error("File not found.");
    } else if !absolute_path.is_file() {
        return error("Path is not a file.");
    }

    let ext = absolute_path.extension().unwrap_or_default();
    let contents = std::fs::read_to_string(&absolute_path)?;
    let lines = contents.split('\n').count();

    if start_line.is_some_and(|v| v > end_line.unwrap_or(usize::MAX)) {
        return error("`start_line` must be less than or equal to `end_line`.");
    } else if start_line.is_some_and(|v| v == 0) {
        return error("`start_line` must be greater than 0.");
    } else if end_line.is_some_and(|v| v == 0) {
        return error("`end_line` must be greater than 0.");
    } else if start_line.is_some_and(|v| v > contents.lines().count()) {
        return error(format!(
            "`start_line` is greater than the number of lines in the file ({lines})."
        ));
    }

    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or(lines).min(lines);
    let width = end.to_string().len();

    // Number every line with its absolute position so the model can feed a
    // range straight back to this tool (or to the line-addressed git tools).
    let mut body = contents
        .split('\n')
        .enumerate()
        .filter(|(idx, _)| {
            let num = idx + 1;
            num >= start && num <= end
        })
        .map(|(idx, line)| format!("{num:>width$}: {line}", num = idx + 1))
        .collect::<Vec<_>>()
        .join("\n");

    if end < lines {
        body.push_str(&format!("\n... (truncated after line #{end}) ..."));
    }

    Ok(indoc::formatdoc! {"
        ```{ext}
        {body}
        ```
    "}
    .into())
}

#[cfg(test)]
#[path = "read_file_tests.rs"]
mod tests;
