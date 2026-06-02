use std::collections::HashMap;

use camino_tempfile::tempdir;

use super::*;

#[tokio::test]
async fn dot_means_workspace_root() {
    // Regression: pre-PR, `paths: ["."]` resolved via `root.join(".")` and
    // walked the workspace. The new validator rejects bare `.` because
    // `clean-path` normalizes it to a `CurDir`-only path. Both grep_files
    // and list_files special-case `.` alongside `""` to preserve the
    // workspace-root sentinel.
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "world").unwrap();

    let matches = fs_grep_files(
        tmp.path(),
        "world".to_owned(),
        None,
        Some(vec![".".to_owned()].into()),
        None,
    )
    .await
    .unwrap();

    assert!(
        matches.contains("hello.txt"),
        "expected match in workspace root, got: {matches}"
    );
}

#[tokio::test]
async fn subdir_scope_respects_root_ignore() {
    // Mirrors the real `docs/.vitepress/dist/` leak: scoping the search to
    // `docs` must not surface files from an `.ignore`-excluded build-output
    // dir nested below it.
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(root.join(".ignore"), "docs/.vitepress/dist/\n").unwrap();

    for (path, content) in [
        ("docs/getting-started.md", "color profile"),
        ("docs/.vitepress/dist/index.html", "color profile"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    let matches = fs_grep_files(
        root,
        "color profile".to_owned(),
        None,
        Some(vec!["docs".to_owned()].into()),
        None,
    )
    .await
    .unwrap()
    .replace('\\', "/");

    assert!(
        matches.contains("docs/getting-started.md"),
        "expected the doc source in results, got: {matches}"
    );
    assert!(
        !matches.contains(".vitepress/dist"),
        "build output must be excluded, got: {matches}"
    );
}

#[tokio::test]
async fn restricts_to_extensions() {
    // The extension filter is how `grep_user_docs` narrows to markdown prose,
    // dropping vitepress build config like `config.mts`.
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    for path in ["docs/guide.md", "docs/config.mts"] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "find me").unwrap();
    }

    let matches = fs_grep_files(
        root,
        "find me".to_owned(),
        None,
        Some(vec!["docs".to_owned()].into()),
        Some(vec!["md".to_owned()].into()),
    )
    .await
    .unwrap()
    .replace('\\', "/");

    assert!(matches.contains("docs/guide.md"), "got: {matches}");
    assert!(!matches.contains("config.mts"), "got: {matches}");
}

#[tokio::test]
async fn rejects_workspace_escape() {
    let tmp = tempdir().unwrap();
    let result = fs_grep_files(
        tmp.path(),
        "anything".to_owned(),
        None,
        Some(vec!["../escape".to_owned()].into()),
        None,
    )
    .await;

    let err = result.expect_err("escape attempt must be a hard error");
    assert!(
        err.to_string().contains("escape the workspace"),
        "unexpected error: {err}"
    );
}

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
            pattern: "hi\"",
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

        let matches = fs_grep_files(root, pattern.to_owned(), Some(5), paths, None)
            .await
            .unwrap()
            .replace('\\', "/");

        assert_eq!(matches, expected.join(""), "test case: {name}");
    }
}
