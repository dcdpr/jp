use std::fs;

use assert_matches::assert_matches;
use camino::Utf8Path;
use camino_tempfile::{Utf8TempDir, tempdir};
use indoc::indoc;
use jp_tool::Action;
use serde_json::{Map, Value};

use super::*;
use crate::util::runner::{ExitCode, ProcessOutput};

fn ctx() -> (Utf8TempDir, Context) {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_path_buf(),
        action: Action::Run,
    };
    (dir, ctx)
}

fn pat(old: &str, new: &str) -> Vec<Pattern> {
    vec![Pattern {
        old: old.to_owned(),
        new: new.to_owned(),
    }]
}

fn answers(key: impl Into<String>, value: impl Into<Value>) -> Map<String, Value> {
    Map::from_iter([(key.into(), value.into())])
}

/// Pre-approved answers for the `apply_changes` inquiry.
fn approved() -> Map<String, Value> {
    answers("apply_changes", true)
}

/// Configurable process runner for the `is_file_dirty` check.
/// `DirtyRunner(true)` reports files as dirty, `DirtyRunner(false)` as clean.
struct DirtyRunner(bool);

impl ProcessRunner for DirtyRunner {
    fn run_with_env_and_stdin(
        &self,
        _program: &str,
        _args: &[&str],
        _working_dir: &Utf8Path,
        _env: &[(&str, &str)],
        _stdin: Option<&str>,
    ) -> Result<ProcessOutput, std::io::Error> {
        let stdout = if self.0 { " M file\n" } else { "" };
        Ok(ProcessOutput {
            stdout: stdout.to_owned(),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    }
}

/// Writes content to a temp file, applies patterns with pre-approved answers,
/// and returns the outcome and resulting file content.
fn run_modify(content: &str, patterns: &[Pattern], regex: bool) -> (Outcome, String) {
    let (result, after) = run_modify_with(content, patterns, regex, &approved());
    (result.unwrap(), after)
}

fn run_modify_with(
    content: &str,
    patterns: &[Pattern],
    regex: bool,
    answers: &Map<String, Value>,
) -> (ToolResult, String) {
    let (_dir, ctx) = ctx();
    let file_path = ctx.root.join("test.txt");
    fs::write(&file_path, content).unwrap();
    let result = fs_modify_file_impl(
        &ctx,
        answers,
        "test.txt",
        patterns,
        regex,
        true,
        true,
        &DirtyRunner(false),
    );
    let after = fs::read_to_string(&file_path).unwrap();
    (result, after)
}

#[test]
fn test_validate_patterns() {
    struct TestCase {
        patterns: Vec<Pattern>,
        expected: Result<(), &'static str>,
    }

    let cases = [
        ("empty", TestCase {
            patterns: vec![],
            expected: Err("No patterns provided."),
        }),
        ("identical_old_new", TestCase {
            patterns: pat("hello", "hello"),
            expected: Err("identical"),
        }),
        ("valid", TestCase {
            patterns: pat("hello", "world"),
            expected: Ok(()),
        }),
    ];

    for (name, tc) in cases {
        let result = validate_patterns(&tc.patterns);
        match (&result, &tc.expected) {
            (Ok(()), Ok(())) => {}
            (Err(msg), Err(substr)) => {
                assert!(msg.contains(substr), "{name}: '{substr}' not in: {msg}");
            }
            _ => panic!("{name}: expected {:?}, got {result:?}", tc.expected),
        }
    }
}

#[test]
fn test_validate_path() {
    let cases = [
        ("absolute", "/absolute/path", Err("Path must be relative.")),
        ("relative", "src/main.rs", Ok(())),
    ];

    for (name, path, expected) in cases {
        let mut path = path.to_owned();
        if cfg!(windows) && path.starts_with('/') {
            path = format!("c:{path}");
        }

        assert_eq!(validate_path(&path), expected, "test case: {name}");
    }
}

#[test]
fn test_pattern_preview() {
    let cases = [
        ("short", "hello", "hello"),
        ("multiline", "first line\nsecond line", "first line"),
    ];

    for (name, input, expected) in cases {
        assert_eq!(pattern_preview(input), expected, "test case: {name}");
    }

    let long = "a".repeat(100);
    let preview = pattern_preview(&long);
    assert!(preview.len() < 65);
    assert!(preview.ends_with("..."));
}

#[test]
fn test_is_broad_change() {
    struct TestCase {
        before: String,
        after: String,
        expected: bool,
    }

    let cases = [
        ("small_file_not_flagged", TestCase {
            before: "a\nb\nc\n".to_owned(),
            after: "x\ny\nz\n".to_owned(),
            expected: false,
        }),
        ("large_file_all_changed", TestCase {
            before: (0..20)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            after: (0..20)
                .map(|i| format!("changed {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            expected: true,
        }),
        ("large_file_few_changed", {
            let mut lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
            let before = lines.join("\n");
            lines[0] = "changed 0".to_owned();
            lines[1] = "changed 1".to_owned();
            let after = lines.join("\n");
            TestCase {
                before,
                after,
                expected: false,
            }
        }),
    ];

    for (name, tc) in cases {
        assert_eq!(
            is_broad_change(&tc.before, &tc.after),
            tc.expected,
            "test case: {name}"
        );
    }
}

#[test]
fn test_find_broad_changes() {
    // Detects broad changes in large files
    let before = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let after = (0..20)
        .map(|i| format!("changed {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let changes = vec![Change {
        path: "test.txt".into(),
        before,
        after,
    }];
    assert_eq!(find_broad_changes(&changes), Some(vec!["test.txt"]));

    // Ignores small files
    let changes = vec![Change {
        path: "test.txt".into(),
        before: "a\nb\nc\n".to_owned(),
        after: "x\ny\nz\n".to_owned(),
    }];
    assert_eq!(find_broad_changes(&changes), None);
}

mod apply_patterns {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_basic_replacements() {
        struct TestCase {
            content: &'static str,
            old: &'static str,
            new: &'static str,
            expected: &'static str,
            outcome: PatternOutcome,
        }

        let cases = [
            ("replace_first_line", TestCase {
                content: "hello world\n",
                old: "hello world",
                new: "hello universe",
                expected: "hello universe\n",
                outcome: PatternOutcome::Applied,
            }),
            ("delete_content", TestCase {
                content: "hello world\n",
                old: "hello world",
                new: "",
                expected: "\n",
                outcome: PatternOutcome::Applied,
            }),
            ("replace_with_multiple_lines", TestCase {
                content: "hello world\n",
                old: "hello world",
                new: "hello\nworld\n",
                expected: "hello\nworld\n\n",
                outcome: PatternOutcome::Applied,
            }),
            ("replace_without_trailing_newline", TestCase {
                content: "hello world\nhello universe",
                old: "hello world",
                new: "hello there",
                expected: "hello there\nhello universe",
                outcome: PatternOutcome::Applied,
            }),
            ("replace_subset_of_line", TestCase {
                content: "hello world how are you doing?",
                old: "world",
                new: "universe",
                expected: "hello universe how are you doing?",
                outcome: PatternOutcome::Applied,
            }),
            ("replace_across_multiple_lines", TestCase {
                content: "hello world\nhow are you doing?",
                old: "world\nhow",
                new: "universe\nwhat",
                expected: "hello universe\nwhat are you doing?",
                outcome: PatternOutcome::Applied,
            }),
            ("pattern_not_found", TestCase {
                content: "hello world how are you doing?",
                old: "universe",
                new: "galaxy",
                expected: "hello world how are you doing?",
                outcome: PatternOutcome::NotFound,
            }),
        ];

        for (name, tc) in cases {
            let result = apply_patterns(
                tc.content.to_owned(),
                &pat(tc.old, tc.new),
                false,
                true,
                true,
            );
            assert_eq!(result.content, tc.expected, "test case: {name}");
            assert_eq!(result.outcomes, vec![tc.outcome], "test case: {name}");
        }
    }

    #[test]
    fn test_sequential() {
        let patterns = vec![
            Pattern {
                old: "bbb".to_owned(),
                new: "xxx".to_owned(),
            },
            Pattern {
                old: "xxx ccc".to_owned(),
                new: "yyy".to_owned(),
            },
        ];

        let result = apply_patterns("aaa bbb ccc".to_owned(), &patterns, false, true, true);
        assert_eq!(result.content, "aaa yyy");
        assert_eq!(result.outcomes, vec![
            PatternOutcome::Applied,
            PatternOutcome::Applied,
        ]);
    }

    #[test]
    fn test_not_found_skipped() {
        let patterns = vec![
            Pattern {
                old: "missing".to_owned(),
                new: "x".to_owned(),
            },
            Pattern {
                old: "world".to_owned(),
                new: "earth".to_owned(),
            },
        ];

        let result = apply_patterns("hello world".to_owned(), &patterns, false, true, true);
        assert_eq!(result.content, "hello earth");
        assert_eq!(result.outcomes, vec![
            PatternOutcome::NotFound,
            PatternOutcome::Applied,
        ]);
    }

    /// Regression: multiline replacement where old and new have the same
    /// number of lines but different content.
    #[test]
    fn test_edge_case_changing_line_count() {
        let old = "/// A tool call response event - the result of executing a tool.\n///\n/// \
                   This event MUST be in response to a `ToolCallRequest` event, with a matching \
                   `id`.\n#[derive(Debug, Clone, PartialEq)]\npub struct ToolCallResponse {\n    \
                   /// ID matching the corresponding ToolCallRequest\n    pub id: String,";

        let new = "/// A tool call response event - the result of executing a tool.\n///\n/// \
                   This event MUST be in response to a `ToolCallRequest` event, with a matching \
                   `id`.\n#[derive(Debug, Clone, PartialEq)]\npub struct ToolCallResponse {\n    \
                   /// ID matching the corresponding `ToolCallRequest`\n    pub id: String,";

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

        let expected = indoc!(
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

        let result = apply_patterns(source.to_owned(), &pat(old, new), false, true, true);
        assert_eq!(result.content, expected);
    }

    /// Regression: pattern with trailing newline should match correctly.
    #[test]
    fn test_edge_case_trailing_newlines() {
        let old = "use crossterm::style::Stylize as _;\nuse inquire::Confirm;\nuse \
                   jp_conversation::{Conversation, ConversationId, ConversationStream};\nuse \
                   jp_format::conversation::DetailsFmt;\nuse jp_printer::Printable as _;\n\nuse \
                   crate::{Output, cmd::Success, ctx::Ctx};\n";

        let new = "use crossterm::style::Stylize as _;\nuse inquire::Confirm;\nuse \
                   jp_conversation::{Conversation, ConversationId, ConversationStream};\nuse \
                   jp_format::conversation::DetailsFmt;\n\nuse crate::{Output, cmd::Success, \
                   ctx::Ctx};\n";

        let source = indoc! {"
            use crossterm::style::Stylize as _;
            use inquire::Confirm;
            use jp_conversation::{Conversation, ConversationId, ConversationStream};
            use jp_format::conversation::DetailsFmt;
            use jp_printer::Printable as _;

            use crate::{Output, cmd::Success, ctx::Ctx};

            #[derive(Debug, clap::Args)]
            pub(crate) struct Rm {
                /// Conversation IDs to remove."};

        let expected = indoc! {"
            use crossterm::style::Stylize as _;
            use inquire::Confirm;
            use jp_conversation::{Conversation, ConversationId, ConversationStream};
            use jp_format::conversation::DetailsFmt;

            use crate::{Output, cmd::Success, ctx::Ctx};

            #[derive(Debug, clap::Args)]
            pub(crate) struct Rm {
                /// Conversation IDs to remove."};

        let result = apply_patterns(source.to_owned(), &pat(old, new), false, true, true);
        assert_eq!(result.content, expected);
    }
}

mod format_pattern_report {
    use super::*;

    #[test]
    fn test_single_success_is_empty() {
        let outcomes = vec![PatternOutcome::Applied];
        assert_eq!(format_pattern_report(&pat("a", "b"), &outcomes), "");
    }

    #[test]
    fn test_multi_all_success() {
        let patterns = vec![
            Pattern {
                old: "a".to_owned(),
                new: "b".to_owned(),
            },
            Pattern {
                old: "c".to_owned(),
                new: "d".to_owned(),
            },
        ];
        let outcomes = vec![PatternOutcome::Applied, PatternOutcome::Applied];
        assert_eq!(
            format_pattern_report(&patterns, &outcomes),
            "2/2 patterns applied."
        );
    }

    #[test]
    fn test_partial_failure() {
        let patterns = vec![
            Pattern {
                old: "a".to_owned(),
                new: "b".to_owned(),
            },
            Pattern {
                old: "missing_pattern".to_owned(),
                new: "d".to_owned(),
            },
        ];
        let outcomes = vec![PatternOutcome::Applied, PatternOutcome::NotFound];
        let report = format_pattern_report(&patterns, &outcomes);
        assert!(report.contains("1/2 patterns applied."), "report: {report}");
        assert!(report.contains("#2:"), "report: {report}");
        assert!(report.contains("missing_pattern"), "report: {report}");
    }
}

mod guard_broad_replacement {
    use super::*;

    #[test]
    fn test_approved() {
        let answers = answers("broad_replacement", true);
        assert!(guard_broad_replacement(&answers, "reject", "question".to_owned()).is_none());
    }

    #[test]
    fn test_rejected() {
        let answers = answers("broad_replacement", false);
        let result = guard_broad_replacement(&answers, "reject msg", "question".to_owned());
        assert_matches!(result.unwrap().unwrap(), Outcome::Error { transient, .. } => {
            assert!(!transient);
        });
    }

    #[test]
    fn test_no_answer() {
        let result = guard_broad_replacement(&Map::new(), "reject", "my question".to_owned());
        assert_matches!(result.unwrap().unwrap(), Outcome::NeedsInput { question } => {
            assert_eq!(question.id, "broad_replacement");
            assert_eq!(question.text, "my question");
            assert_eq!(question.default, Some(Value::Bool(false)));
        });
    }
}

mod find_blocked_regex_patterns {
    use super::*;

    #[test]
    fn test_detects_known_patterns() {
        for pattern in BLOCKED_REGEX_PATTERNS {
            let patterns = pat(pattern, "replacement");
            let blocked = find_blocked_regex_patterns(&patterns);
            assert_eq!(blocked, Some(vec![*pattern]), "expected blocked: {pattern}");
        }
    }

    #[test]
    fn test_allows_specific_regex() {
        assert_eq!(
            find_blocked_regex_patterns(&pat(r"fn\s+\w+", "replacement")),
            None
        );
    }

    #[test]
    fn test_trims_whitespace() {
        assert_eq!(
            find_blocked_regex_patterns(&pat("  .*  ", "replacement")),
            Some(vec![".*"])
        );
    }
}

mod content {
    use super::*;

    mod find {
        use super::*;

        #[test]
        fn test_exact_substring() {
            let c = Content("hello world".to_owned());
            assert_eq!(c.find_exact_substring("world"), Some((6, 11)));
        }

        #[test]
        fn test_trimmed_substring() {
            let c = Content("hello world".to_owned());
            assert_eq!(c.find_trimmed_substring("  world  "), Some((6, 11)));
        }

        #[test]
        fn test_fuzzy_substring() {
            let c = Content("hello    world".to_owned());
            assert!(c.find_fuzzy_substring("hello world").is_some());
        }

        #[test]
        fn test_fuzzy_substring_multiline() {
            let c = Content("hello\nworld".to_owned());
            assert!(c.find_fuzzy_substring("hello\nworld").is_some());
        }

        #[test]
        fn test_pattern_range_tries_all_strategies() {
            let c = Content("hello    world".to_owned());
            assert!(c.find_pattern_range("hello world").is_some());
        }

        #[test]
        fn test_pattern_range_skips_fuzzy_for_multiline() {
            let c = Content("hello   world".to_owned());
            assert_eq!(c.find_pattern_range("hello\nworld"), None);
        }
    }

    #[test]
    fn test_case_sensitivity() {
        struct TestCase {
            content: &'static str,
            old: &'static str,
            new: &'static str,
            case_sensitive: bool,
            use_regex: bool,
            expected: Result<&'static str, &'static str>,
        }

        let cases = [
            ("literal_case_sensitive", TestCase {
                content: "Hello World",
                old: "hello",
                new: "hi",
                case_sensitive: true,
                use_regex: false,
                expected: Err("Cannot find pattern"),
            }),
            ("literal_case_insensitive", TestCase {
                content: "Hello World",
                old: "hello",
                new: "hi",
                case_sensitive: false,
                use_regex: false,
                expected: Ok("hi World"),
            }),
            ("regexp_case_sensitive", TestCase {
                content: "Hello World",
                old: "hello",
                new: "hi",
                case_sensitive: true,
                use_regex: true,
                expected: Ok("Hello World"),
            }),
            ("regexp_case_insensitive", TestCase {
                content: "Hello World",
                old: "hello",
                new: "hi",
                case_sensitive: false,
                use_regex: true,
                expected: Ok("hi World"),
            }),
        ];

        for (name, tc) in cases {
            let c = Content(tc.content.to_owned());
            let result = if tc.use_regex {
                c.replace_regexp(tc.old, tc.new, false, tc.case_sensitive)
            } else {
                c.replace_literal(tc.old, tc.new, false, tc.case_sensitive)
            };

            match (&result, &tc.expected) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual, expected, "test case: {name}");
                }
                (Err(actual), Err(substr)) => {
                    assert!(actual.to_string().contains(substr), "test case: {name}");
                }
                _ => panic!("{name}: expected {:?}, got {result:?}", tc.expected),
            }
        }
    }

    #[test]
    fn test_replace_all_vs_first() {
        struct TestCase {
            content: &'static str,
            old: &'static str,
            new: &'static str,
            replace_all: bool,
            use_regex: bool,
            expected: &'static str,
        }

        let cases = [
            ("literal_all", TestCase {
                content: "foo bar foo baz",
                old: "foo",
                new: "qux",
                replace_all: true,
                use_regex: false,
                expected: "qux bar qux baz",
            }),
            ("literal_first_only", TestCase {
                content: "foo bar foo baz",
                old: "foo",
                new: "qux",
                replace_all: false,
                use_regex: false,
                expected: "qux bar foo baz",
            }),
            ("regexp_all", TestCase {
                content: "foo bar foo baz",
                old: r"\bfoo\b",
                new: "qux",
                replace_all: true,
                use_regex: true,
                expected: "qux bar qux baz",
            }),
            ("regexp_first_only", TestCase {
                content: "foo bar foo baz",
                old: r"\bfoo\b",
                new: "qux",
                replace_all: false,
                use_regex: true,
                expected: "qux bar foo baz",
            }),
        ];

        for (name, tc) in cases {
            let c = Content(tc.content.to_owned());
            let result = if tc.use_regex {
                c.replace_regexp(tc.old, tc.new, tc.replace_all, true)
            } else {
                c.replace_literal(tc.old, tc.new, tc.replace_all, true)
            };
            assert_eq!(result.unwrap(), tc.expected, "test case: {name}");
        }
    }
}

mod fs_modify_file {
    use super::*;

    #[test]
    fn test_simple() {
        let (outcome, after) = run_modify("hello world\n", &pat("hello", "goodbye"), false);
        assert_eq!(after, "goodbye world\n");
        assert_matches!(outcome, Outcome::Success { .. });
    }

    #[test]
    fn test_regex_replacements() {
        struct TestCase {
            content: &'static str,
            old: &'static str,
            new: &'static str,
            expected_content: &'static str,
        }

        let cases = [
            ("capture_group", TestCase {
                content: "hello world\n",
                old: r"(\w+)\s\w+",
                new: "$1 universe",
                expected_content: "hello universe\n",
            }),
            ("delete_via_regex", TestCase {
                content: "hello world\n",
                old: "h(.+?)d\n",
                new: "$1",
                expected_content: "ello worl",
            }),
        ];

        for (name, tc) in cases {
            let (outcome, after) = run_modify(tc.content, &pat(tc.old, tc.new), true);
            assert_eq!(after, tc.expected_content, "test case: {name}");
            assert_matches!(outcome, Outcome::Success { content } => {
                assert!(content.contains("File modified successfully:"), "{name}: {content}");
            });
        }
    }

    #[test]
    fn test_confirmation_flow() {
        let (_dir, ctx) = ctx();
        let file = "test.txt";
        fs::write(ctx.root.join(file), "Hello World").unwrap();

        // Without apply_changes answer -> NeedsInput
        let result = fs_modify_file_impl(
            &Context {
                root: ctx.root.clone(),
                action: Action::Run,
            },
            &Map::new(),
            file,
            &pat("World", "There"),
            true,
            true,
            true,
            &DirtyRunner(false),
        )
        .unwrap();

        assert_matches!(result, Outcome::NeedsInput { question } => {
            assert_eq!(question.id, "apply_changes");
            assert_eq!(question.default, Some(Value::Bool(true)));
        });

        // With apply_changes: true -> Success
        let result = fs_modify_file_impl(
            &ctx,
            &approved(),
            file,
            &pat("World", "There"),
            true,
            true,
            true,
            &DirtyRunner(false),
        )
        .unwrap();

        assert_matches!(result, Outcome::Success { content } => {
            assert!(content.contains("File modified successfully:"));
        });
    }

    #[test]
    fn test_multiple_patterns() {
        struct TestCase {
            content: &'static str,
            patterns: Vec<(&'static str, &'static str)>,
            expected_content: &'static str,
            expected_fragments: Vec<&'static str>,
        }

        let cases = [
            ("all_match", TestCase {
                content: "hello world\ngoodbye moon\n",
                patterns: vec![("hello", "hi"), ("moon", "sun")],
                expected_content: "hi world\ngoodbye sun\n",
                expected_fragments: vec!["2/2 patterns applied."],
            }),
            ("partial_match", TestCase {
                content: "hello world\n",
                patterns: vec![("hello", "hi"), ("nonexistent", "replacement")],
                expected_content: "hi world\n",
                expected_fragments: vec!["1/2 patterns applied.", "#2:"],
            }),
            ("none_match", TestCase {
                content: "hello world\n",
                patterns: vec![("nonexistent", "a"), ("also_nonexistent", "b")],
                expected_content: "hello world\n",
                expected_fragments: vec!["0/2 patterns applied.", "#1:", "#2:"],
            }),
        ];

        for (name, tc) in cases {
            let patterns: Vec<Pattern> = tc
                .patterns
                .iter()
                .map(|(old, new)| Pattern {
                    old: old.to_string(),
                    new: new.to_string(),
                })
                .collect();

            let (outcome, after) = run_modify(tc.content, &patterns, false);
            assert_eq!(after, tc.expected_content, "test case: {name}");
            assert_matches!(outcome, Outcome::Success { content } => {
                for fragment in &tc.expected_fragments {
                    assert!(
                        content.contains(fragment),
                        "{name}: expected '{fragment}' in: {content}"
                    );
                }
            });
        }
    }

    #[test]
    fn test_validation_rejects_identical_patterns() {
        let (outcome, _) = run_modify("hello", &pat("hello", "hello"), false);
        assert_matches!(outcome, Outcome::Error { message, .. } => {
            assert!(message.contains("identical old and new"), "message: {message}");
        });
    }

    #[test]
    fn test_blocked_regex_flow() {
        let (_dir, ctx) = ctx();
        let file = "test.txt";
        fs::write(ctx.root.join(file), "hello\nworld\n").unwrap();

        // No answer -> NeedsInput for broad_replacement
        let result = fs_modify_file_impl(
            &Context {
                root: ctx.root.clone(),
                action: Action::Run,
            },
            &Map::new(),
            file,
            &pat(".*", "replaced"),
            true,
            true,
            true,
            &DirtyRunner(false),
        )
        .unwrap();

        assert_matches!(result, Outcome::NeedsInput { question } => {
            assert_eq!(question.id, "broad_replacement");
            assert_eq!(question.default, Some(Value::Bool(false)));
        });

        // Rejected -> Error
        let result = fs_modify_file_impl(
            &Context {
                root: ctx.root.clone(),
                action: Action::Run,
            },
            &answers("broad_replacement", false),
            file,
            &pat(".*", "replaced"),
            true,
            true,
            true,
            &DirtyRunner(false),
        )
        .unwrap();

        assert_matches!(result, Outcome::Error { message, transient, .. } => {
            assert!(message.contains("overly broad"), "message: {message}");
            assert!(!transient);
        });

        // Accepted -> proceeds
        let mut both = answers("broad_replacement", true);
        both.insert("apply_changes".to_string(), Value::Bool(true));
        let result = fs_modify_file_impl(
            &ctx,
            &both,
            file,
            &pat(".*", "replaced"),
            true,
            true,
            true,
            &DirtyRunner(false),
        )
        .unwrap();

        assert_matches!(result, Outcome::Success { .. });
    }

    #[test]
    fn test_specific_regex_no_confirmation() {
        let (outcome, _) = run_modify("hello world\n", &pat(r"(\w+)\s\w+", "$1 universe"), true);
        assert_matches!(outcome, Outcome::Success { .. });
    }

    #[test]
    fn test_broad_change_triggers_confirmation() {
        let content = (0..20)
            .map(|i| format!("line {i} content"))
            .collect::<Vec<_>>()
            .join("\n");

        let (outcome, _) = run_modify(&content, &pat("content", "stuff"), false);
        assert_matches!(outcome, Outcome::NeedsInput { question } => {
            assert_eq!(question.id, "broad_replacement");
            assert_eq!(question.default, Some(Value::Bool(false)));
        });
    }

    #[test]
    fn test_dirty_file_flow() {
        let (_dir, ctx) = ctx();
        let file = "test.txt";
        fs::write(ctx.root.join(file), "hello world").unwrap();

        // No answer + dirty file -> NeedsInput for modify_dirty_file
        let result = fs_modify_file_impl(
            &Context {
                root: ctx.root.clone(),
                action: Action::Run,
            },
            &approved(),
            file,
            &pat("hello", "goodbye"),
            false,
            true,
            true,
            &DirtyRunner(true),
        )
        .unwrap();

        assert_matches!(result, Outcome::NeedsInput { question } => {
            assert_eq!(question.id, "modify_dirty_file");
            assert_eq!(question.default, None);
        });

        // Rejected -> Err
        let mut rejected = approved();
        rejected.insert("modify_dirty_file".to_string(), Value::Bool(false));
        let result = fs_modify_file_impl(
            &Context {
                root: ctx.root.clone(),
                action: Action::Run,
            },
            &rejected,
            file,
            &pat("hello", "goodbye"),
            false,
            true,
            true,
            &DirtyRunner(true),
        );

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("uncommitted changes")
        );

        // Accepted -> proceeds
        let mut accepted = approved();
        accepted.insert("modify_dirty_file".to_string(), Value::Bool(true));
        let result = fs_modify_file_impl(
            &Context {
                root: ctx.root.clone(),
                action: Action::Run,
            },
            &accepted,
            file,
            &pat("hello", "goodbye"),
            false,
            true,
            true,
            &DirtyRunner(true),
        )
        .unwrap();

        assert_matches!(result, Outcome::Success { .. });

        let after = fs::read_to_string(ctx.root.join(file)).unwrap();
        assert_eq!(after, "goodbye world");
    }

    #[test]
    fn test_broad_change_small_file_no_confirmation() {
        let content = (0..5)
            .map(|i| format!("line {i} content"))
            .collect::<Vec<_>>()
            .join("\n");

        let (outcome, _) = run_modify(&content, &pat("content", "stuff"), false);
        assert_matches!(outcome, Outcome::Success { .. });
    }
}
