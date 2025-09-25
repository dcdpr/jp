// TODO:
//
// Look into using (parts of) <https://github.com/jbr/semantic-edit-mcp> for
// semantic edits with (in-memory) staged changes.

use std::{
    fs::{self, File},
    io::Read as _,
    ops::{Deref, DerefMut, Range},
    path::PathBuf,
};

use super::utils::is_file_dirty;
use crate::Error;

pub struct Content(String);

impl Deref for Content {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Content {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Content {
    fn find_lines_to_replace(&self, pattern: &str) -> Option<Range<usize>> {
        let start_idx = self
            .find_exact_start_line(pattern)
            .or_else(|| self.find_trimmed_start_line(pattern))
            .or_else(|| self.find_fuzzy_start_line(pattern))?;

        Some(start_idx..start_idx + pattern.lines().count())
    }

    fn find_exact_start_line(&self, pattern: &str) -> Option<usize> {
        self.0.find(pattern)
    }

    fn find_trimmed_start_line(&self, pattern: &str) -> Option<usize> {
        self.0.find(pattern.trim())
    }

    fn find_fuzzy_start_line(&self, pattern: &str) -> Option<usize> {
        let first_line_to_replace = pattern
            .lines()
            .next()?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        let first_line_matches =
            self.0
                .lines()
                .enumerate()
                .fold::<Option<Vec<_>>, _>(None, |mut acc, (i, line)| {
                    let fuzzy_line = line.split_whitespace().collect::<Vec<_>>().join(" ");
                    if fuzzy_line.contains(&first_line_to_replace) {
                        acc.get_or_insert_default().push(i);
                    }
                    acc
                })?;

        // TODO: Handle multiple matches
        first_line_matches.first().copied()
    }
}

pub(crate) async fn fs_modify_file(
    root: PathBuf,
    path: String,
    string_to_replace: String,
    new_string: Option<String>,
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
    let mut content = String::new();
    file.read_to_string(&mut content)?;

    let contents = Content(content);
    let mut lines: Vec<String> = contents.lines().map(str::to_owned).collect();

    let change_lines = contents
        .find_lines_to_replace(&string_to_replace)
        .ok_or("Cannot find lines to replace")?;

    // Remove the lines to be replaced
    lines.drain(change_lines.start..change_lines.end.min(lines.len()));

    // Insert new content if any
    if let Some(new_string) = &new_string
        && !new_string.trim().is_empty()
    {
        let new_lines: Vec<String> = new_string.lines().map(str::to_owned).collect();
        for (i, line) in new_lines.into_iter().enumerate() {
            if lines.len() <= change_lines.start + i {
                lines.push(line);
            } else {
                lines[change_lines.start + i] = line;
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
            start_content: &'static str,
            string_to_replace: &'static str,
            new_string: Option<&'static str>,
            final_content: &'static str,
            output: Result<&'static str, &'static str>,
        }

        let cases = vec![
            ("replace first line", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: Some("hello universe"),
                final_content: "hello universe\n",
                output: Ok("File modified successfully."),
            }),
            ("delete first line", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: None,
                final_content: "\n",
                output: Ok("File modified successfully."),
            }),
            ("replace first line with multiple lines", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: Some("hello\nworld\n"),
                final_content: "hello\nworld\n",
                output: Ok("File modified successfully."),
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

            let actual = fs_modify_file(
                root,
                file_path.to_owned(),
                test_case.string_to_replace.to_owned(),
                test_case.new_string.map(str::to_owned),
            )
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
