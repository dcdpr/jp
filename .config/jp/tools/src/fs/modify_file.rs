// TODO:
//
// Look into using (parts of) <https://github.com/jbr/semantic-edit-mcp> for
// semantic edits with (in-memory) staged changes.

use std::{
    fmt::{self, Write as _},
    fs::{self},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::style::{ContentStyle, Stylize as _};
use fancy_regex::RegexBuilder;
use jp_tool::{AnswerType, Outcome, Question};
use serde_json::{Map, Value};
use similar::{ChangeTag, TextDiff, udiff::UnifiedDiff};

use super::utils::is_file_dirty;
use crate::{
    Context, Error,
    util::{ToolResult, error, fail},
};

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
) -> ToolResult {
    if string_to_replace == new_string {
        return error("String to replace is the same as the new string.");
    }

    let p = PathBuf::from(&path);

    if p.is_absolute() {
        return error("Path must be relative.");
    }

    if p.iter().any(|c| c.len() > 30) {
        return error("Individual path components must be less than 30 characters long.");
    }

    if p.iter().count() > 20 {
        return error("Path must be less than 20 components long.");
    }

    let absolute_path = ctx.root.join(path.trim_start_matches('/'));

    let mut changes = vec![];
    for entry in glob::glob(&absolute_path.to_string_lossy())? {
        let entry = entry?;
        if !entry.exists() {
            return error("File does not exist.");
        }

        if !entry.is_file() {
            return error("Path is not a regular file.");
        }

        let Ok(path) = entry.strip_prefix(&ctx.root) else {
            return fail("Path is not within workspace root.");
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
        Ok(format_changes(changes).into())
    } else {
        apply_changes(changes, &ctx.root, answers)
    }
}

fn format_changes(changes: Vec<Change>) -> String {
    changes
        .into_iter()
        .map(|change| {
            let path = change.path.to_string_lossy();

            let diff = text_diff(&change.before, &change.after);
            let unified = unified_diff(&diff, &path);
            let colored = colored_diff(&diff, &unified);

            format!("{path}:\n\n```diff\n{colored}\n```")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn apply_changes(
    changes: Vec<Change>,
    root: &Path,
    answers: &Map<String, Value>,
) -> Result<Outcome, Error> {
    let mut modified = vec![];
    let count = changes.len();
    for Change {
        path,
        after,
        before,
    } in changes
    {
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

        let file_path = path.to_string_lossy();
        let file_path = file_path.trim_start_matches('/');
        let absolute_path = root.join(file_path);

        fs::write(absolute_path, &after)?;

        let diff = text_diff(&before, &after);
        let diff = unified_diff(&diff, file_path);

        modified.push(diff.to_string());
    }

    Ok(format!(
        "{} modified successfully:\n\n{}",
        if count == 1 { "File" } else { "Files" },
        modified
            .into_iter()
            .map(|diff| format!("```diff\n{diff}```"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
    .into())
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

fn text_diff<'old, 'new, 'bufs>(
    old: &'old str,
    new: &'new str,
) -> TextDiff<'old, 'new, 'bufs, str> {
    similar::TextDiff::configure()
        .algorithm(similar::Algorithm::Patience)
        .timeout(Duration::from_secs(2))
        .diff_lines(old, new)
}

fn unified_diff<'diff, 'old, 'new, 'bufs>(
    diff: &'diff TextDiff<'old, 'new, 'bufs, str>,
    file: &str,
) -> UnifiedDiff<'diff, 'old, 'new, 'bufs, str> {
    let mut unified = diff.unified_diff();
    unified.context_radius(3).header(file, file);
    unified
}

fn colored_diff<'old, 'new, 'diff: 'old + 'new, 'bufs>(
    diff: &'diff TextDiff<'old, 'new, 'bufs, str>,
    unified: &UnifiedDiff<'diff, 'old, 'new, 'bufs, str>,
) -> String {
    let mut buf = String::new();
    for hunk in unified.iter_hunks() {
        for op in hunk.ops() {
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
        }

        let cases = vec![
            ("replace_first_line", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: "hello universe",
            }),
            ("delete_first_line", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: "",
            }),
            ("replace_first_line_with_multiple_lines", TestCase {
                start_content: "hello world\n",
                string_to_replace: "hello world",
                new_string: "hello\nworld\n",
            }),
            ("replace_whole_line_without_newline", TestCase {
                start_content: "hello world\nhello universe",
                string_to_replace: "hello world",
                new_string: "hello there",
            }),
            ("replace_subset_of_line", TestCase {
                start_content: "hello world how are you doing?",
                string_to_replace: "world",
                new_string: "universe",
            }),
            ("replace_subset_across_multiple_lines", TestCase {
                start_content: "hello world\nhow are you doing?",
                string_to_replace: "world\nhow",
                new_string: "universe\nwhat",
            }),
            ("ignore_replacement_if_no_match", TestCase {
                start_content: "hello world how are you doing?",
                string_to_replace: "universe",
                new_string: "galaxy",
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
            .map(|v| v.into_content().unwrap_or_default())
            .map_err(|e| e.to_string());

            let response = match &actual {
                Ok(v) => v,
                Err(e) => e,
            };

            insta::with_settings!({
                snapshot_suffix => name,
                omit_expression => true,
                prepend_module_to_snapshot => false,
            }, {
                insta::assert_snapshot!(&response);

                let file_content = fs::read_to_string(&absolute_file_path).unwrap();
                insta::assert_snapshot!(&file_content);
            });
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
                output: Ok("File modified successfully:\n\n```diff\n--- test.txt\n+++ \
                            test.txt\n@@ -1 +1 @@\n-hello world\n+hello universe\n```"),
            }),
            ("delete", TestCase {
                start_content: "hello world\n",
                string_to_replace: "h(.+?)d\n",
                new_string: "$1",
                final_content: "ello worl",
                output: Ok("File modified successfully:\n\n```diff\n--- test.txt\n+++ \
                            test.txt\n@@ -1 +1 @@\n-hello world\n+ello worl\n\\ No newline at \
                            end of file\n```"),
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

            match (actual, test_case.output) {
                (Ok(Outcome::Success { content }), Ok(expected)) => {
                    assert_eq!(&content, expected, "test case: {name}");
                }
                (actual, expected) => {
                    assert_eq!(
                        actual,
                        expected.map(Into::into).map_err(str::to_owned),
                        "test case: {name}"
                    );
                }
            }

            assert_eq!(
                &fs::read_to_string(&absolute_file_path).unwrap(),
                test_case.final_content,
                "test case: {name}"
            );
        }
    }
}
