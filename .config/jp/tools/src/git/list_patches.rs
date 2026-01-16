// New Workflow:
//
// 1. use `git_patches` to list all patches with a unique ID (base64 encoded patch)
// 2. use `git_patch` to apply one or more patches based on IDs
//
// TODO: Don't use Base64, since the IDs are too large. Instead use a hash of
// the patch, but require `git_patch` to provide a file path to apply one or
// more patch IDs to. Then, we iterate over all hunks in that file, and compare
// the hash of the hunk against the patch IDs to find the matching patches to
// apply.

use std::path::Path;

use duct::cmd;
use serde::Serialize;

use crate::{
    to_simple_xml_with_root,
    util::{OneOrMany, ToolResult, error},
};

#[derive(Debug, Serialize)]
struct Patch {
    path: String,
    id: String,
    diff: String,
}

pub(crate) fn git_list_patches(root: &Path, files: OneOrMany<String>) -> ToolResult {
    let mut patches = vec![];

    for path in files {
        let path = path.trim();
        let file_content = std::fs::read_to_string(root.join(path)).unwrap_or_default();
        let source_lines: Vec<&str> = file_content.lines().collect();

        let output = cmd!(
            "git",
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            path,
        )
        .dir(root)
        .unchecked()
        .stdout_capture()
        .stderr_capture()
        .run()?;

        let stdout = String::from_utf8(output.stdout).unwrap_or_default();
        let stderr = String::from_utf8(output.stderr).unwrap_or_default();

        if !output.status.success() {
            return error(format!(
                "Failed to list patches for path '{path}': {stderr}"
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
            // let id1 = BASE64_URL_SAFE_NO_PAD.encode(path.as_bytes());
            // let id2 = seahash::hash(hunk_with_header.as_bytes());

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
    // 1. Parse the Header to find coordinates
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

    // 2. Calculate Context Indices (0-indexed)
    let line_idx = if start_line > 0 { start_line - 1 } else { 0 };

    // 3 lines before
    let ctx_before_start = line_idx.saturating_sub(3);
    let ctx_before_end = line_idx;

    // 3 lines after
    let hunk_end_idx = line_idx + count;
    let ctx_after_start = hunk_end_idx;
    let ctx_after_end = std::cmp::min(source_lines.len(), hunk_end_idx + 3);

    let mut result = String::new();

    // A. Pre-context
    for i in ctx_before_start..ctx_before_end {
        if let Some(line) = source_lines.get(i) {
            result.push(' ');
            result.push_str(line);
            result.push('\n');
        }
    }

    // B. Actual Changes
    // Skip the first line of raw_body, which contains the header info (e.g., "-1,1 +1,1 @@")
    let body_lines: Vec<&str> = hunk.lines().collect();
    for line in body_lines.iter().skip(1) {
        result.push_str(line);
        result.push('\n');
    }

    // C. Post-context
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
    use std::{fs, process::Command};

    use jp_tool::Outcome;
    use tempfile::TempDir;

    use super::*;

    /// Helper function to run git commands in the test directory
    fn run_git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("Failed to execute git command");

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("Git command {args:?} failed: {stderr}");
        }
    }

    #[test]
    fn test_git_list_patches_success() {
        // Skip if git is not installed
        //
        // TODO: use DI to inject a mock
        if which::which("git").is_err() {
            return;
        }

        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let root = temp_dir.path();
        let filename = "test_script.rs";

        // Setup
        {
            let file_path = root.join(filename);

            // 2. Initialize Git Repo
            run_git(root, &["init"]);

            if std::env::var("CI").is_ok() {
                run_git(root, &["config", "user.email", "you@example.com"]);
                run_git(root, &["config", "user.name", "Your Name"]);
            }

            // 3. Create initial file state and commit
            let initial_content = "fn main() {\n    {};\n    println!(\"Hello\");\n}\n";
            fs::write(&file_path, initial_content).expect("Failed to write file");
            run_git(root, &["add", filename]);
            run_git(root, &["commit", "-m", "Initial commit"]);

            // 4. Modify file to create a 'dirty' state (the patch)
            let modified_content =
                "fn main() -> () {\n    {};\n    println!(\"Hello World\");\n}\n";
            fs::write(&file_path, modified_content).expect("Failed to update file");
        }

        match git_list_patches(root, vec![filename.to_string()].into()) {
            Ok(tool_output) => {
                let Outcome::Success { content } = tool_output else {
                    panic!("Unexpected ToolResult: {tool_output:?}");
                };

                assert_eq!(content, indoc::indoc! {r#"
                    <patches>
                      <patch>
                        <path>test_script.rs</path>
                        <id>0</id>
                        <diff><![CDATA[
                    -fn main() {
                    +fn main() -> () {
                         {};
                         println!("Hello World");
                     }
                    ]]></diff>
                      </patch>
                      <patch>
                        <path>test_script.rs</path>
                        <id>1</id>
                        <diff><![CDATA[
                     fn main() -> () {
                         {};
                    -    println!("Hello");
                    +    println!("Hello World");
                     }
                    ]]></diff>
                      </patch>
                    </patches>"#
                });
            }
            Err(e) => panic!("git_list_patches returned an error: {e:?}"),
        }
    }
}
