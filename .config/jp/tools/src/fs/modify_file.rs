// TODO:
//
// Look into using (parts of) <https://github.com/jbr/semantic-edit-mcp> for
// semantic edits with (in-memory) staged changes.

use std::{
    fmt::{self, Write as _},
    fs::{self},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use crossterm::style::{ContentStyle, Stylize as _};
use fancy_regex::RegexBuilder;
use jp_tool::{AnswerType, Outcome, Question};
use serde_json::{Map, Value};
use similar::{ChangeTag, TextDiff};

use super::utils::is_file_dirty;
use crate::{Context, Error};

pub struct Change {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

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
    if string_to_replace == new_string {
        return Err("String to replace is the same as the new string.".into());
    }

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

    let mut changes = vec![];
    for entry in glob::glob(&absolute_path.to_string_lossy())? {
        let entry = entry?;
        if !entry.exists() {
            return Err("File does not exist.".into());
        }

        if !entry.is_file() {
            return Err("Path is not a regular file.".into());
        }

        let Ok(path) = entry.strip_prefix(&ctx.root) else {
            return Err("Path is not within workspace root.".into());
        };

        let before = fs::read_to_string(&entry)?;
        let contents = Content(before);

        let after = if replace_using_regex {
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

        changes.push(Change {
            path: path.to_path_buf(),
            before: contents.0,
            after,
        });
    }

    if ctx.format_parameters {
        Ok(format_changes(changes, &ctx.root).into())
    } else {
        apply_changes(changes, &ctx.root, answers)
    }
}

fn format_changes(changes: Vec<Change>, root: &Path) -> String {
    changes
        .into_iter()
        .map(|change| {
            let path = root.join(change.path.to_string_lossy().trim_start_matches('/'));
            let diff = file_diff(&change.before, &change.after);
            format!("{}:\n\n```diff\n{diff}\n```", path.display())
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn apply_changes(
    changes: Vec<Change>,
    root: &Path,
    answers: &Map<String, Value>,
) -> Result<Outcome, Error> {
    let modified = changes
        .iter()
        .map(|c| c.path.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    for Change { path, after, .. } in changes {
        if is_file_dirty(root, &path)? {
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
                            text: format!(
                                "File '{}' has uncommitted changes. Modify anyway?",
                                path.display()
                            ),
                            answer_type: AnswerType::Boolean,
                            default: Some(Value::Bool(false)),
                        },
                    });
                }
            }
        }

        let absolute_path = root.join(path.to_string_lossy().trim_start_matches('/'));

        fs::write(absolute_path, after)?;
    }

    Ok(format!("File(s) modified successfully:\n\n{}.", modified.join("\n")).into())
}

struct Line(Option<usize>);

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => write!(f, "    "),
            Some(idx) => write!(f, "{:<4}", idx + 1),
        }
    }
}

fn file_diff(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);

    let mut buf = String::new();
    for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if idx > 0 {
            println!("{:-^1$}", "-", 80);
        }
        for op in group {
            for change in diff.iter_inline_changes(op) {
                let (sign, s) = match change.tag() {
                    ChangeTag::Delete => ("-", ContentStyle::new().red()),
                    ChangeTag::Insert => ("+", ContentStyle::new().green()),
                    ChangeTag::Equal => (" ", ContentStyle::new().dim()),
                };
                let _ = write!(
                    &mut buf,
                    "{}{} |{}",
                    s.apply(Line(change.old_index())),
                    s.apply(Line(change.new_index())),
                    s.apply(sign).bold(),
                );
                for (emphasized, value) in change.iter_strings_lossy() {
                    if emphasized {
                        let _ = write!(&mut buf, "{}", s.apply(value).underlined().on_black());
                    } else {
                        let _ = write!(&mut buf, "{}", s.apply(value));
                    }
                }
                if change.missing_newline() {
                    buf.push('\n');
                }
            }
        }
    }

    buf.push_str("".reset().to_string().as_str());
    buf
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

            let ctx = Context {
                root,
                format_parameters: false,
            };

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

        let ctx = Context {
            root,
            format_parameters: false,
        };

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

            let ctx = Context {
                root,
                format_parameters: false,
            };

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
