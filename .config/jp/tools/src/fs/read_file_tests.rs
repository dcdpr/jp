use camino_tempfile::tempdir;
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
            expected: "```txt\n... (starting from line #2) ...\nbar\n... (truncated after line \
                       #2) ...\n```\n"
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
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("file.txt");

        std::fs::write(&path, file_contents).unwrap();

        let result = fs_read_file(tmp.path(), "file.txt".to_owned(), start_line, end_line)
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
