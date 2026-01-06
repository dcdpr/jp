use std::{ffi::OsStr, path::PathBuf};

use crate::util::{ToolResult, error};

pub(crate) async fn fs_read_file(
    root: PathBuf,
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> ToolResult {
    let absolute_path = root.join(path.trim_start_matches('/'));
    if !absolute_path.exists() {
        return error("File not found.");
    } else if !absolute_path.is_file() {
        return error("Path is not a file.");
    }

    let ext = absolute_path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default();

    let mut contents = std::fs::read_to_string(&absolute_path)?;

    if let Some(start_line) = start_line {
        contents = contents.split('\n').skip(start_line).collect::<String>();
        contents.insert_str(0, &format!("... (starting from line #{start_line}) ...\n"));
    }

    if let Some(end_line) = end_line {
        contents = contents.split('\n').take(end_line).collect::<String>();
        contents.push_str(&format!("\n... (truncated after line #{end_line}) ..."));
    }

    Ok(indoc::formatdoc! {"
        ```{ext}
        {contents}
        ```
    "}
    .into())
}
