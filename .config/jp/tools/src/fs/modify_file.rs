use std::{
    fs::{self, File},
    io::Read as _,
    path::PathBuf,
};

use super::utils::is_file_dirty;
use crate::Error;

#[derive(serde::Deserialize)]
pub struct Change {
    start_line: usize,
    lines_to_replace: usize,
    new_content: String,
}

pub(crate) async fn fs_modify_file(
    root: PathBuf,
    path: String,
    changes: Vec<Change>,
) -> std::result::Result<String, Error> {
    let p = PathBuf::from(&path);

    if p.is_absolute() {
        return Err("Path must be relative.".into());
    }

    if p.iter().any(|c| c.len() > 30) {
        return Err("Individual path components must be less than 30 characters long.".into());
    }

    if p.iter().count() > 20 {
        return Err("Path must be less than 20 components long.".into());
    }

    let absolute_path = root.join(path.trim_start_matches('/'));

    if !absolute_path.exists() {
        return Err("File does not exist.".into());
    }

    if !absolute_path.is_file() {
        return Err("Path is not a regular file.".into());
    }

    if is_file_dirty(&root, &p)? {
        return Err("File has uncommitted changes. Please commit or discard first.".into());
    }

    // Read existing file content
    let mut file = File::open(&absolute_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let mut lines: Vec<String> = contents.lines().map(str::to_owned).collect();
    let total_lines = lines.len();

    // Validate all changes
    for change in &changes {
        if change.start_line == 0 {
            return Err("Line numbers are 1-indexed, got 0.".into());
        }
        if change.start_line > total_lines + 1 {
            return Err(format!(
                "start_line {} exceeds file length {}",
                change.start_line, total_lines
            )
            .into());
        }
        if change.start_line + change.lines_to_replace > total_lines + 1 {
            return Err("Change would extend beyond file length".into());
        }
    }

    // Sort changes by start_line in descending order
    let mut sorted_changes = changes;
    sorted_changes.sort_by(|a, b| b.start_line.cmp(&a.start_line));

    // Check for overlapping changes
    for i in 0..sorted_changes.len() {
        for j in i + 1..sorted_changes.len() {
            let change_a = &sorted_changes[i];
            let change_b = &sorted_changes[j];

            let a_start = change_a.start_line;
            let a_end = change_a.start_line + change_a.lines_to_replace;
            let b_start = change_b.start_line;
            let b_end = change_b.start_line + change_b.lines_to_replace;

            // Check if ranges overlap
            if a_start < b_end && b_start < a_end {
                return Err("Overlapping changes are not allowed.".into());
            }
        }
    }

    // Apply changes from last to first
    for change in sorted_changes {
        let start_idx = change.start_line.saturating_sub(1);
        let end_idx = start_idx + change.lines_to_replace;

        // Remove the lines to be replaced
        lines.drain(start_idx..end_idx.min(lines.len()));

        // Insert new content if any
        if !change.new_content.is_empty() {
            let new_lines: Vec<String> = change.new_content.lines().map(str::to_owned).collect();
            for (i, line) in new_lines.into_iter().enumerate() {
                lines.insert(start_idx + i, line);
            }
        }
    }

    // Write modified content back to file
    let new_contents = lines.join("\n");
    if !new_contents.is_empty() || !contents.is_empty() {
        // Preserve trailing newline if original had one
        let final_contents = if contents.ends_with('\n') && !new_contents.ends_with('\n') {
            format!("{new_contents}\n")
        } else {
            new_contents
        };

        fs::write(&absolute_path, final_contents)?;
    }

    Ok("File modified successfully.".into())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn test_modify_file_replace_word() {
        struct TestCase {
            changes: Vec<Change>,
            output: Result<&'static str, &'static str>,
            start_content: &'static str,
            final_content: &'static str,
        }

        let cases = vec![
            ("replace first line", TestCase {
                changes: vec![Change {
                    start_line: 1,
                    lines_to_replace: 1,
                    new_content: "hello universe".to_string(),
                }],
                output: Ok("File modified successfully."),
                start_content: "hello world\n",
                final_content: "hello universe\n",
            }),
            ("delete first line", TestCase {
                changes: vec![Change {
                    start_line: 1,
                    lines_to_replace: 1,
                    new_content: String::new(),
                }],
                output: Ok("File modified successfully."),
                start_content: "hello world\n",
                final_content: "\n",
            }),
        ];

        for (name, test_case) in cases {
            // Create root directory.
            let temp_dir = tempdir().unwrap();
            let root = temp_dir.path().to_path_buf();

            // Create file to be modified.
            let file_path = "test.txt";
            let absolute_file_path = root.join(file_path);
            fs::write(&absolute_file_path, test_case.start_content).unwrap();

            let actual = fs_modify_file(root, file_path.to_owned(), test_case.changes)
                .await
                .map_err(|e| e.to_string());

            assert_eq!(
                actual,
                test_case.output.map(str::to_owned).map_err(str::to_owned),
                "test case: {name}"
            );

            assert_eq!(
                &fs::read_to_string(&absolute_file_path).unwrap(),
                test_case.final_content,
                "test case: {name}"
            );
        }
    }
}
