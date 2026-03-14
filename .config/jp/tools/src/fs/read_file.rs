use camino::Utf8Path;

use crate::util::{ToolResult, error};

pub(crate) async fn fs_read_file(
    root: &Utf8Path,
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

    let ext = absolute_path.extension().unwrap_or_default();
    let mut contents = std::fs::read_to_string(&absolute_path)?;
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

    if let Some(start_line) = start_line {
        contents = contents
            .split('\n')
            .skip(start_line - 1)
            .collect::<Vec<_>>()
            .join("\n");

        contents.insert_str(0, &format!("... (starting from line #{start_line}) ...\n"));
    }

    if let Some(end_line) = end_line {
        contents = contents
            .split('\n')
            .take(end_line)
            .collect::<Vec<_>>()
            .join("\n");

        contents.push_str(&format!("\n... (truncated after line #{end_line}) ..."));
    }

    Ok(indoc::formatdoc! {"
        ```{ext}
        {contents}
        ```
    "}
    .into())
}

#[cfg(test)]
#[path = "read_file_tests.rs"]
mod tests;
