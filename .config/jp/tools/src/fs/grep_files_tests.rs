use std::collections::HashMap;

use camino_tempfile::tempdir;

use super::*;

#[tokio::test]
#[test_log::test]
async fn test_grep_files() {
    struct TestCase {
        pattern: &'static str,
        paths: Vec<&'static str>,
        given: Vec<(&'static str, &'static str)>,
        expected: Vec<&'static str>,
    }

    let cases = HashMap::from([
        ("pattern", TestCase {
            pattern: "hi",
            paths: vec!["test/a.txt"],
            given: vec![("test/a.txt", "hello\nhi\ngoodbye")],
            expected: vec![
                "test/a.txt-1-hello\n",
                "test/a.txt:2:hi\n",
                "test/a.txt-3-goodbye\n",
            ],
        }),
        ("dont-return-entire-file", TestCase {
            pattern: "1|2|3",
            paths: vec!["test/a.txt"],
            given: vec![("test/a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9")],
            expected: vec![
                "test/a.txt:1:1\n",
                "test/a.txt:2:2\n",
                "test/a.txt:3:3\n",
                "test/a.txt-4-4\n",
                "test/a.txt-5-5\n",
                "test/a.txt-6-6\n",
                "test/a.txt-7-7\n",
                "test/a.txt-8-8\n",
            ],
        }),
        ("multiple-files", TestCase {
            pattern: "1|2|3",
            paths: vec!["test/a.txt", "test/b.txt"],
            given: vec![
                ("test/a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9"),
                ("test/b.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9"),
            ],
            expected: vec![
                "test/a.txt:1:1\n",
                "test/a.txt:2:2\n",
                "test/a.txt:3:3\n",
                "test/a.txt-4-4\n",
                "test/a.txt-5-5\n",
                "test/a.txt-6-6\n",
                "test/a.txt-7-7\n",
                "test/a.txt-8-8\n",
                "test/b.txt:1:1\n",
                "test/b.txt:2:2\n",
                "test/b.txt:3:3\n",
                "test/b.txt-4-4\n",
                "test/b.txt-5-5\n",
                "test/b.txt-6-6\n",
                "test/b.txt-7-7\n",
                "test/b.txt-8-8\n",
            ],
        }),
        ("multiple-files", TestCase {
            pattern: "foo",
            paths: vec![],
            given: vec![("test/a.txt", "foo"), ("test/b.txt", "bar")],
            expected: vec!["test/a.txt:1:foo\n"],
        }),
        ("search-in-subdir", TestCase {
            pattern: "foo",
            paths: vec!["test/subdir"],
            given: vec![
                ("test/a.txt", "baz"),
                ("test/b.txt", "bar"),
                ("test/subdir/c.txt", "foo"),
            ],
            expected: vec!["test/subdir/c.txt:1:foo\n"],
        }),
        ("escape-double-quote", TestCase {
            pattern: r#"hi\""#,
            paths: vec!["test/a.txt"],
            given: vec![("test/a.txt", "hello\nhi\ngoodbye")],
            expected: vec![
                "test/a.txt-1-hello\n",
                "test/a.txt:2:hi\n",
                "test/a.txt-3-goodbye\n",
            ],
        }),
    ]);

    for (
        name,
        TestCase {
            pattern,
            paths,
            given,
            expected,
        },
    ) in cases
    {
        let tmp = tempdir().unwrap();
        let root = tmp.path();

        for (path, content) in given {
            let path = root.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }

        let paths = (!paths.is_empty()).then_some(paths.into_iter().map(str::to_owned).collect());

        let matches = fs_grep_files(root, pattern.to_owned(), Some(5), paths)
            .await
            .unwrap();

        assert_eq!(matches, expected.join(""), "test case: {name}");
    }
}
