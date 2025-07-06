use std::{ffi::OsStr, path::PathBuf};

use crate::Error;

pub(crate) async fn fs_read_file(
    root: PathBuf,
    path: String,
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

    let contents = std::fs::read_to_string(&absolute_path)?;

    Ok(indoc::formatdoc! {"
        ```{ext}
        {contents}
        ```
    "})
}
