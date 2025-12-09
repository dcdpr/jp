// TODO:
//
// Look into using (parts of) <https://github.com/jbr/semantic-edit-mcp> for
// semantic edits with (in-memory) staged changes.

use std::{
    fs::{self, File},
    io::Read as _,
    ops::{Deref, DerefMut},
    path::PathBuf,
};

use fancy_regex::RegexBuilder;
use jp_tool::{AnswerType, Outcome, Question};
use serde_json::{Map, Value};

use super::utils::is_file_dirty;
use crate::{Context, Error};

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
    fn find_pattern_range(&self, pattern: &str) -> Option<(usize, usize)> {
        self.find_exact_substring(pattern)
            .or_else(|| self.find_trimmed_substring(pattern))
            .or_else(|| {
                // Only use fuzzy matching for single-line patterns.
                // Multi-line fuzzy matching is unreliable because the pattern length
                // may not match the actual matched text length due to different line wrapping.
                if pattern.lines().count() <= 1 {
                    self.find_fuzzy_substring(pattern)
                } else {
                    None
                }
            })
    }

    fn find_exact_substring(&self, pattern: &str) -> Option<(usize, usize)> {
        let start = self.0.find(pattern)?;
        Some((start, start + pattern.len()))
    }

    fn find_trimmed_substring(&self, pattern: &str) -> Option<(usize, usize)> {
        let trimmed_pattern = pattern.trim();
        let start = self.0.find(trimmed_pattern)?;
        Some((start, start + trimmed_pattern.len()))
    }

    fn find_fuzzy_substring(&self, pattern: &str) -> Option<(usize, usize)> {
        let first_line_to_find = pattern
            .lines()
            .next()?
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        // Find lines that fuzzy match
        let mut byte_offset = 0;
        for line in self.0.lines() {
            let fuzzy_line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if fuzzy_line.contains(&first_line_to_find) {
                return Some((byte_offset, byte_offset + pattern.len()));
            }
            byte_offset += line.len() + 1; // +1 for newline
        }
        None
    }

    fn replace_using_regexp(
        &self,
        find: &str,
        replace: &str,
    ) -> std::result::Result<String, Error> {
        let re = RegexBuilder::new(find)
            .case_insensitive(true)
            .multi_line(true)
            .dot_matches_new_line(true)
            .unicode_mode(true)
            .build()?;

        Ok(re.replace_all(&self.0, replace).to_string())
    }
}

pub(crate) async fn fs_modify_file(
    ctx: Context,
    answers: &Map<String, Value>,
    path: String,
    string_to_replace: String,
    new_string: String,
    replace_using_regex: bool,
) -> std::result::Result<Outcome, Error> {
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

    let absolute_path = ctx.root.join(path.trim_start_matches('/'));

    let mut modified_files = vec![];
    for entry in glob::glob(&absolute_path.to_string_lossy())? {
        let entry = entry?;
        if !entry.exists() {
            return Err("File does not exist.".into());
        }

        if !entry.is_file() {
            return Err("Path is not a regular file.".into());
        }

        if is_file_dirty(&ctx.root, &p)? {
            match answers.get("modify_dirty_file").and_then(Value::as_bool) {
                Some(true) => {}
                Some(false) => {
                    return Err(
                        "File has uncommitted changes. Please commit or discard first.".into(),
                    );
                }
                None => {
                    return Ok(Outcome::NeedsInput {
                        question: Question {
                            id: "modify_dirty_file".to_string(),
                            text: format!("File '{path}' has uncommitted changes. Modify anyway?"),
                            answer_type: AnswerType::Boolean,
                            default: Some(Value::Bool(false)),
                        },
                    });
                }
            }
        }

        // Read existing file content
        let mut file = File::open(&absolute_path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;

        let contents = Content(content);

        let new_content = if replace_using_regex {
            contents.replace_using_regexp(&string_to_replace, &new_string)?
        } else {
            let (start_byte, mut end_byte) = contents
                .find_pattern_range(&string_to_replace)
                .ok_or("Cannot find pattern to replace")?;

            // Check if pattern is followed by a newline
            let followed_by_newline =
                end_byte < contents.len() && contents.as_bytes()[end_byte] == b'\n';

            // If followed by newline, consume it
            if followed_by_newline {
                end_byte += 1;
            }

            // Replace the pattern with new string
            let mut new_content = String::new();
            new_content.push_str(&contents[..start_byte]);
            new_content.push_str(&new_string);

            // If we consumed a newline but replacement doesn't end with one, add it
            // back
            if followed_by_newline && !new_string.ends_with('\n') {
                new_content.push('\n');
            }

            new_content.push_str(&contents[end_byte..]);
            new_content
        };

        // Write modified content back to file
        fs::write(&absolute_path, new_content)?;

        if let Ok(relative_path) = absolute_path.strip_prefix(&ctx.root) {
            modified_files.push(relative_path.to_string_lossy().to_string());
        }
    }

    Ok(format!(
        "File(s) modified successfully:\n\n{}.",
        modified_files.join("\n")
    )
    .into())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use indoc::indoc;
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    #[test_log::test]
    async fn test_modify_file_replace_word() {
        struct TestCase {
            start_content: &'static str,
            string_to_replace: &'static str,
            new_string: &'static str,
            final_content: &'static str,
            output: Result<&'static str, &'static str>,
        }

        let cases = vec![
            ("replace first line", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: "hello universe",
                final_content: "hello universe\n",
                output: Ok("File(s) modified successfully:\n\ntest.txt."),
            }),
            ("delete first line", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: "",
                final_content: "\n",
                output: Ok("File(s) modified successfully:\n\ntest.txt."),
            }),
            ("replace first line with multiple lines", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: "hello\nworld\n",
                final_content: "hello\nworld\n",
                output: Ok("File(s) modified successfully:\n\ntest.txt."),
            }),
            ("replace whole line without newline", TestCase {
                start_content: "hello world\nhello universe",
                string_to_replace: "hello world",
                new_string: "hello there",
                final_content: "hello there\nhello universe",
                output: Ok("File(s) modified successfully:\n\ntest.txt."),
            }),
            ("replace subset of line", TestCase {
                start_content: "hello world how are you doing?",
                string_to_replace: "world",
                new_string: "universe",
                final_content: "hello universe how are you doing?",
                output: Ok("File(s) modified successfully:\n\ntest.txt."),
            }),
            ("replace subset across multiple lines", TestCase {
                start_content: "hello world\nhow are you doing?",
                string_to_replace: "world\nhow",
                new_string: "universe\nwhat",
                final_content: "hello universe\nwhat are you doing?",
                output: Ok("File(s) modified successfully:\n\ntest.txt."),
            }),
            ("ignore replacement if no match", TestCase {
                start_content: "hello world how are you doing?",
                string_to_replace: "universe",
                new_string: "galaxy",
                final_content: "hello world how are you doing?",
                output: Err("Cannot find pattern to replace"),
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

            let ctx = Context { root };

            let actual = fs_modify_file(
                ctx,
                &Map::new(),
                file_path.to_owned(),
                test_case.string_to_replace.to_owned(),
                test_case.new_string.to_owned(),
                false,
            )
            .await
            .map_err(|e| e.to_string());

            assert_eq!(
                actual,
                test_case.output.map(Into::into).map_err(str::to_owned),
                "test case: {name}"
            );

            assert_eq!(
                &fs::read_to_string(&absolute_file_path).unwrap(),
                test_case.final_content,
                "test case: {name}"
            );
        }
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_issue_with_changing_number_of_lines() {
        // {
        //   "path": "crates/jp_conversation/src/event/tool_call.rs",
        //   "string_to_replace": "/// A tool call response event - the result of executing a tool.\n///\n/// This event MUST be in response to a `ToolCallRequest` event, with a matching `id`.\n#[derive(Debug, Clone, PartialEq)]\npub struct ToolCallResponse {\n    /// ID matching the corresponding ToolCallRequest\n    pub id: String,",
        //   "new_string": "/// A tool call response event - the result of executing a tool.\n///\n/// This event MUST be in response to a `ToolCallRequest` event, with a matching `id`.\n#[derive(Debug, Clone, PartialEq)]\npub struct ToolCallResponse {\n    /// ID matching the corresponding `ToolCallRequest`\n    pub id: String,"
        // }

        let string_to_replace = "/// A tool call response event - the result of executing a \
                                 tool.\n///\n/// This event MUST be in response to a \
                                 `ToolCallRequest` event, with a matching `id`.\n#[derive(Debug, \
                                 Clone, PartialEq)]\npub struct ToolCallResponse {\n    /// ID \
                                 matching the corresponding ToolCallRequest\n    pub id: String,";

        let new_string = "/// A tool call response event - the result of executing a \
                          tool.\n///\n/// This event MUST be in response to a `ToolCallRequest` \
                          event, with a matching `id`.\n#[derive(Debug, Clone, PartialEq)]\npub \
                          struct ToolCallResponse {\n    /// ID matching the corresponding \
                          `ToolCallRequest`\n    pub id: String,";

        let source = indoc!(
            "
            /// A tool call response event - the result of executing a tool.
            ///
            /// This event MUST be in response to a `ToolCallRequest` event, with a matching `id`.
            #[derive(Debug, Clone, PartialEq)]
            pub struct ToolCallResponse {
                /// ID matching the corresponding ToolCallRequest
                pub id: String,

                /// The result of executing the tool: Ok(content) on success, Err(error) on
                /// failure
                pub result: Result<String, String>,
            }"
        );

        let result = indoc!(
            "
            /// A tool call response event - the result of executing a tool.
            ///
            /// This event MUST be in response to a `ToolCallRequest` event, with a matching `id`.
            #[derive(Debug, Clone, PartialEq)]
            pub struct ToolCallResponse {
                /// ID matching the corresponding `ToolCallRequest`
                pub id: String,

                /// The result of executing the tool: Ok(content) on success, Err(error) on
                /// failure
                pub result: Result<String, String>,
            }"
        );

        // Create root directory.
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();

        // Create file to be modified.
        let file_path = "test.txt";
        let absolute_file_path = root.join(file_path);
        fs::write(&absolute_file_path, source).unwrap();

        let ctx = Context { root };

        let _actual = fs_modify_file(
            ctx,
            &Map::new(),
            file_path.to_owned(),
            string_to_replace.to_owned(),
            new_string.to_owned(),
            false,
        )
        .await
        .map_err(|e| e.to_string());

        assert_eq!(&fs::read_to_string(&absolute_file_path).unwrap(), result,);
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_modify_file_replace_with_regexp() {
        struct TestCase {
            start_content: &'static str,
            string_to_replace: &'static str,
            new_string: &'static str,
            final_content: &'static str,
            output: Result<&'static str, &'static str>,
        }

        let cases = vec![
            ("capture group", TestCase {
                start_content: "hello world\n",
                string_to_replace: r"(\w+)\s\w+",
                new_string: "$1 universe",
                final_content: "hello universe\n",
                output: Ok("File(s) modified successfully:\n\ntest.txt."),
            }),
            ("delete", TestCase {
                start_content: "hello world\n",
                string_to_replace: "h(.+?)d\n",
                new_string: "$1",
                final_content: "ello worl",
                output: Ok("File(s) modified successfully:\n\ntest.txt."),
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

            let ctx = Context { root };

            let actual = fs_modify_file(
                ctx,
                &Map::new(),
                file_path.to_owned(),
                test_case.string_to_replace.to_owned(),
                test_case.new_string.to_owned(),
                true,
            )
            .await
            .map_err(|e| e.to_string());

            assert_eq!(
                actual,
                test_case.output.map(Into::into).map_err(str::to_owned),
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
