use camino::Utf8Path;
use serde::Serialize;

use crate::{
    to_simple_xml_with_root,
    util::{
        OneOrMany, ToolResult, error,
        runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
    },
};

#[derive(Debug, Serialize)]
struct Patch {
    path: String,
    id: String,
    diff: String,
}

pub(crate) fn git_list_patches(root: &Utf8Path, files: OneOrMany<String>) -> ToolResult {
    git_list_patches_impl(root, files, &DuctProcessRunner)
}

fn git_list_patches_impl<R: ProcessRunner>(
    root: &Utf8Path,
    files: OneOrMany<String>,
    runner: &R,
) -> ToolResult {
    let mut patches = vec![];

    for path in files {
        let path = path.trim();
        let file = root.join(path);
        if !file.is_file() {
            return error(format!("File not found: {file}"));
        }

        let file_content = std::fs::read_to_string(file).unwrap_or_default();
        let source_lines: Vec<&str> = file_content.lines().collect();

        let ProcessOutput {
            stdout,
            stderr,
            status,
        } = runner.run(
            "git",
            &["diff-files", "-p", "--minimal", "--unified=0", "--", path],
            root,
        )?;

        if !status.is_success() {
            return error(format!(
                "Failed to list patches for path '{path}': {stderr}",
            ));
        }

        // See: <https://www.gnu.org/software/diffutils/manual/diffutils.html#Detailed-Unified>
        let Some((_, tail)) = stdout.split_once("\n@@ ") else {
            // Ignore file without changes.
            continue;
        };

        let mut tail = tail.to_string();
        tail.insert_str(0, "@@ ");

        for (id, hunk) in tail.split("\n@@ ").enumerate() {
            let hunk_with_header = format!("@@ {hunk}");

            patches.push(Patch {
                path: path.to_string(),
                id: id.to_string(),
                diff: pretty_print_diff(&hunk_with_header, hunk, &source_lines),
            });
        }
    }

    to_simple_xml_with_root(&patches, "patches").map(Into::into)
}

/// Pretty print a git diff hunk.
///
/// This produces non-valid diff output, but we use IDs to match the actual
/// valid diff, and use this as a visual aid to help understand the diff.
fn pretty_print_diff(hunk_with_header: &str, hunk: &str, source_lines: &[&str]) -> String {
    // Parse the Header to find coordinates
    let parts: Vec<&str> = hunk_with_header.split_whitespace().collect();

    // Find part starting with '+' (target file coordinates)
    let new_file_part = parts.iter().find(|p| p.starts_with('+')).unwrap_or(&"+0,0");
    let coords: Vec<&str> = new_file_part.trim_start_matches('+').split(',').collect();

    let start_line: usize = coords[0].parse().unwrap_or(0);
    let count: usize = if coords.len() > 1 {
        coords[1].parse().unwrap_or(0)
    } else {
        1
    };

    // Calculate Context Indices (0-indexed)
    let line_idx = if start_line > 0 { start_line - 1 } else { 0 };

    // 3 lines before
    let ctx_before_start = line_idx.saturating_sub(3);
    let ctx_before_end = line_idx;

    // 3 lines after
    let hunk_end_idx = line_idx + count;
    let ctx_after_start = hunk_end_idx;
    let ctx_after_end = std::cmp::min(source_lines.len(), hunk_end_idx + 3);

    let mut result = String::new();

    // Pre-context
    for i in ctx_before_start..ctx_before_end {
        if let Some(line) = source_lines.get(i) {
            result.push(' ');
            result.push_str(line);
            result.push('\n');
        }
    }

    // Actual Changes
    // Skip the first line of raw_body, which contains the header info (e.g.,
    // "-1,1 +1,1 @@")
    let body_lines: Vec<&str> = hunk.lines().collect();
    for line in body_lines.iter().skip(1) {
        result.push_str(line);
        result.push('\n');
    }

    // Post-context
    for i in ctx_after_start..ctx_after_end {
        if let Some(line) = source_lines.get(i) {
            result.push(' ');
            result.push_str(line);
            result.push('\n');
        }
    }

    result.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use camino_tempfile::tempdir;
    use jp_tool::Outcome;

    use super::*;
    use crate::util::runner::MockProcessRunner;

    #[test]
    fn test_git_list_patches_multiple_hunks() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        let filename = "test_script.rs";

        let modified_content = "fn main() -> () {\n    {};\n    println!(\"Hello World\");\n}\n";
        fs::write(root.join(filename), modified_content).unwrap();

        // Mock git diff output
        let mock_diff = indoc::indoc! {r#"
            diff --git a/test_script.rs b/test_script.rs
            index 1234567..abcdefg 100644
            --- a/test_script.rs
            +++ b/test_script.rs
            @@ -1 +1 @@
            -fn main() {
            +fn main() -> () {
            @@ -3 +3 @@
            -    println!("Hello");
            +    println!("Hello World");
        "#};

        let runner = MockProcessRunner::success(mock_diff);
        let content = git_list_patches_impl(root, vec![filename.to_string()].into(), &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(content, indoc::indoc! {r#"
            <patches>
                <patch>
                    <path>test_script.rs</path>
                    <id>0</id>
                    <diff>
                        -fn main() {
                        +fn main() -> () {
                             {};
                             println!("Hello World");
                         }
                    </diff>
                </patch>
                <patch>
                    <path>test_script.rs</path>
                    <id>1</id>
                    <diff>
                         fn main() -> () {
                             {};
                        -    println!("Hello");
                        +    println!("Hello World");
                         }
                    </diff>
                </patch>
            </patches>"#
        });
    }

    #[test]
    fn test_git_list_patches_single_hunk() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        let filename = "simple.rs";
        let file_path = root.join(filename);

        let content = "fn foo() -> i32 {\n    42\n}\n";
        fs::write(&file_path, content).unwrap();

        let mock_diff = indoc::indoc! {r"
            diff --git a/simple.rs b/simple.rs
            index abc123..def456 100644
            --- a/simple.rs
            +++ b/simple.rs
            @@ -2 +2 @@
            -    0
            +    42
        "};

        let runner = MockProcessRunner::success(mock_diff);
        let content = git_list_patches_impl(root, vec!["simple.rs".to_string()].into(), &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(content, indoc::indoc! {"
            <patches>
                <patch>
                    <path>simple.rs</path>
                    <id>0</id>
                    <diff>
                         fn foo() -> i32 {
                        -    0
                        +    42
                         }
                    </diff>
                </patch>
            </patches>"
        });
    }

    #[test]
    fn test_git_list_patches_no_changes() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        let filename = "unchanged.rs";
        let file_path = root.join(filename);

        fs::write(&file_path, "fn main() {}\n").unwrap();

        let runner = MockProcessRunner::success("");
        let content = git_list_patches_impl(root, vec![filename.to_string()].into(), &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(content, "<patches>\n</patches>");
    }

    #[test]
    fn test_git_list_patches_git_command_fails() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        let filename = "error.rs";
        let file_path = root.join(filename);

        fs::write(&file_path, "fn main() {}\n").unwrap();

        let runner = MockProcessRunner::error("fatal: not a git repository");

        let outcome =
            git_list_patches_impl(root, vec![filename.to_string()].into(), &runner).unwrap();
        let Outcome::Error { message, .. } = outcome else {
            panic!("Expected error but got: {outcome:?}");
        };

        assert!(message.contains("Failed to list patches"));
        assert!(message.contains("fatal: not a git repository"));
    }

    #[test]
    fn test_git_list_patches_context_lines() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        let filename = "context.rs";

        // File with enough lines to test context
        let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
        fs::write(root.join(filename), content).unwrap();

        // Change in the middle (line 5)
        let mock_diff = indoc::indoc! {r"
            diff --git a/context.rs b/context.rs
            index abc..def 100644
            --- a/context.rs
            +++ b/context.rs
            @@ -5 +5 @@
            -line5
            +MODIFIED
        "};

        let runner = MockProcessRunner::success(mock_diff);
        let content = git_list_patches_impl(root, vec![filename.to_string()].into(), &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(content, indoc::indoc! {"
            <patches>
                <patch>
                    <path>context.rs</path>
                    <id>0</id>
                    <diff>
                         line2
                         line3
                         line4
                        -line5
                        +MODIFIED
                         line6
                         line7
                         line8
                    </diff>
                </patch>
            </patches>"
        });
    }
}
