use std::{ffi::OsStr, path::PathBuf};

use crate::Error;

pub(crate) async fn fs_read_file(
    root: PathBuf,
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> std::result::Result<String, Error> {
    let absolute_path = root.join(path.trim_start_matches('/'));
    if !absolute_path.exists() {
        return Err("File not found.".into());
    } else if !absolute_path.is_file() {
        return Err("Path is not a file.".into());
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
    "})
}
