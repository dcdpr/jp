use std::collections::HashMap;

use assert_matches::assert_matches;
use camino_tempfile::tempdir;

use super::*;

#[tokio::test]
#[test_log::test]
async fn test_list_files() {
    struct TestCase {
        prefixes: Vec<&'static str>,
        extensions: Vec<&'static str>,
        given: Vec<&'static str>,
        expected: Vec<&'static str>,
    }

    let cases = HashMap::from([
        ("sorted", TestCase {
            prefixes: vec![],
            extensions: vec![],
            given: vec!["test/a.txt", "test/b.txt"],
            expected: vec!["test/a.txt", "test/b.txt"],
        }),
        ("prefixed", TestCase {
            prefixes: vec!["test2"],
            extensions: vec![],
            given: vec!["test/a.txt", "test2/b.txt"],
            expected: vec!["test2/b.txt"],
        }),
        ("multiple-prefixes", TestCase {
            prefixes: vec!["one", "two"],
            extensions: vec![],
            given: vec!["one/a.txt", "two/b.txt", "nope/c.txt"],
            expected: vec!["one/a.txt", "two/b.txt"],
        }),
        ("extension", TestCase {
            prefixes: vec![],
            extensions: vec!["txt"],
            given: vec!["test/a.txt", "test/b.txt", "test/c.md"],
            expected: vec!["test/a.txt", "test/b.txt"],
        }),
        ("extension-multiple", TestCase {
            prefixes: vec![],
            extensions: vec!["rs", "md"],
            given: vec!["test/a.rs", "test/b.txt", "test/c.md"],
            expected: vec!["test/a.rs", "test/c.md"],
        }),
        ("nested-files", TestCase {
            prefixes: vec![],
            extensions: vec![],
            given: vec!["test/b.txt", "test/c.md", "test/d/e.txt"],
            expected: vec!["test/b.txt", "test/c.md", "test/d/e.txt"],
        }),
        ("partial-prefix", TestCase {
            prefixes: vec!["rfd/D"],
            extensions: vec![],
            given: vec!["rfd/D01-foo.md", "rfd/D02-bar.md", "rfd/001-baz.md"],
            expected: vec!["rfd/D01-foo.md", "rfd/D02-bar.md"],
        }),
        ("partial-prefix-with-extension", TestCase {
            prefixes: vec!["rfd/D"],
            extensions: vec!["md"],
            given: vec!["rfd/D01-foo.md", "rfd/D02-bar.txt", "rfd/001-baz.md"],
            expected: vec!["rfd/D01-foo.md"],
        }),
        ("partial-prefix-nested", TestCase {
            prefixes: vec!["src/foo"],
            extensions: vec![],
            given: vec!["src/foo.rs", "src/foo_tests.rs", "src/bar.rs"],
            expected: vec!["src/foo.rs", "src/foo_tests.rs"],
        }),
    ]);

    for (
        name,
        TestCase {
            prefixes,
            extensions,
            given,
            expected,
        },
    ) in cases
    {
        let tmp = tempdir().unwrap();
        let root = tmp.path();

        for path in given {
            let path = root.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, "").unwrap();
        }

        let prefixes =
            (!prefixes.is_empty()).then_some(prefixes.into_iter().map(str::to_owned).collect());

        let extensions =
            (!extensions.is_empty()).then_some(extensions.into_iter().map(str::to_owned).collect());

        let files = fs_list_files(root, prefixes, extensions).await.unwrap();

        assert_eq!(
            files
                .into_files()
                .into_iter()
                .map(|s| s.replace('\\', "/"))
                .collect::<Vec<_>>(),
            expected,
            "test case: {name}"
        );
    }
}

#[tokio::test]
async fn dot_prefix_lists_workspace_root() {
    // Regression: pre-PR, `prefixes: ["."]` walked the workspace via
    // `root.join(".")`. The new validator rejects bare `.`, so the
    // workspace-root sentinel needs to be honored alongside `""`.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("a.txt"), "").unwrap();
    std::fs::write(root.join("b.txt"), "").unwrap();

    let files = fs_list_files(root, Some(vec![".".to_owned()].into()), None)
        .await
        .unwrap();

    let mut listed = files.into_files();
    listed.sort();
    assert_eq!(listed, vec!["a.txt".to_owned(), "b.txt".to_owned()]);
}

#[tokio::test]
#[test_log::test]
async fn test_empty_list() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let files = fs_list_files(root, Some(vec!["foo".to_owned()].into()), None)
        .await
        .unwrap();

    assert_matches!(files, Files::Empty);
}
