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
mod tests {
    use jp_tool::Outcome;

    use super::*;

    #[tokio::test]
    async fn test_fs_read_file() {
        struct TestCase {
            file_contents: String,
            start_line: Option<usize>,
            end_line: Option<usize>,
            expected: String,
        }

        let cases = vec![
            ("all content", TestCase {
                file_contents: "foo\nbar\nbaz\n".to_owned(),
                start_line: None,
                end_line: None,
                expected: "```txt\nfoo\nbar\nbaz\n\n```\n".to_owned(),
            }),
            ("start line", TestCase {
                file_contents: "foo\nbar\nbaz\n".to_owned(),
                start_line: Some(2),
                end_line: None,
                expected: "```txt\n... (starting from line #2) ...\nbar\nbaz\n\n```\n".to_owned(),
            }),
            ("end line", TestCase {
                file_contents: "foo\nbar\nbaz\n".to_owned(),
                start_line: None,
                end_line: Some(2),
                expected: "```txt\nfoo\nbar\n... (truncated after line #2) ...\n```\n".to_owned(),
            }),
            ("start and end line", TestCase {
                file_contents: "foo\nbar\nbaz\n\n".to_owned(),
                start_line: Some(2),
                end_line: Some(2),
                expected: "```txt\n... (starting from line #2) ...\nbar\n... (truncated after \
                           line #2) ...\n```\n"
                    .to_owned(),
            }),
        ];

        for (
            name,
            TestCase {
                file_contents,
                start_line,
                end_line,
                expected,
            },
        ) in cases
        {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("file.txt");

            std::fs::write(&path, file_contents).unwrap();

            let result = fs_read_file(
                tmp.path().to_owned(),
                "file.txt".to_owned(),
                start_line,
                end_line,
            )
            .await
            .unwrap();

            let out = match result {
                Outcome::Success { content } => content,
                Outcome::Error { message, .. } => message,
                Outcome::NeedsInput { .. } => String::new(),
            };

            assert_eq!(out, expected, "failed test case '{name}'");
        }
    }
}
