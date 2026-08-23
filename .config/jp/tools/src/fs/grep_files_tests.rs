use std::collections::HashMap;

use camino_tempfile::tempdir;
use ignore::gitignore::Gitignore;

use super::{super::utils::suppress_matcher, *};

#[tokio::test]
async fn grep_with_restricted_policy_skips_ungranted_files() {
    use jp_tool::{AccessPolicy, FsRule};

    let ws = tempdir().unwrap();
    std::fs::create_dir(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), "needle in src").unwrap();
    std::fs::write(ws.path().join("secret.txt"), "needle in secret").unwrap();

    // Only `src` is readable; grep over the whole workspace must not search
    // `secret.txt`.
    let policy = AccessPolicy {
        fs: vec![FsRule::new("src").with_read(true)],
        ..AccessPolicy::default()
    };
    let matches = fs_grep_files(
        ws.path(),
        Some(&policy),
        "needle".to_owned(),
        None,
        None,
        None,
        &Gitignore::empty(),
    )
    .await
    .unwrap()
    .replace('\\', "/");

    assert!(matches.contains("src/lib.rs"), "got: {matches}");
    assert!(
        !matches.contains("secret"),
        "ungranted file searched: {matches}"
    );
}

#[tokio::test]
async fn grep_skips_binary_files() {
    let ws = tempdir().unwrap();
    std::fs::write(ws.path().join("lib.rs"), "needle in source").unwrap();
    // An archive or object file: NUL bytes early, and the pattern present in a
    // symbol name. Searching it would emit undecodable bytes.
    std::fs::write(ws.path().join("libjp.a"), b"!<arch>\x00\x01needle\xff\xfe").unwrap();

    let matches = fs_grep_files(
        ws.path(),
        None,
        "needle".to_owned(),
        None,
        None,
        None,
        &Gitignore::empty(),
    )
    .await
    .unwrap();

    assert!(matches.contains("lib.rs"), "got: {matches}");
    assert!(!matches.contains("libjp.a"), "binary searched: {matches}");
}

#[cfg(unix)]
#[tokio::test]
async fn greps_files_under_approved_external_mount() {
    use std::os::unix::fs::symlink;

    use jp_tool::{AccessPolicy, FsRule};

    let ws = tempdir().unwrap();
    let ext = tempdir().unwrap();
    let ext_canon = ext.path().canonicalize_utf8().unwrap();
    std::fs::write(ext_canon.join("a.rs"), "the needle is here").unwrap();
    symlink(ext.path(), ws.path().join("fork")).unwrap();

    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("fork")
                .with_external(true)
                .with_approved_target(Some(ext_canon))
                .with_read(true),
        ],
        ..AccessPolicy::default()
    };
    let matches = fs_grep_files(
        ws.path(),
        Some(&policy),
        "needle".to_owned(),
        None,
        Some(vec!["fork".to_owned()].into()),
        None,
        &Gitignore::empty(),
    )
    .await
    .unwrap();

    assert!(matches.contains("needle"), "got: {matches}");
    assert!(matches.contains("fork/a.rs"), "path missing: {matches}");
}

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
        None,
        "world".to_owned(),
        None,
        Some(vec![".".to_owned()].into()),
        None,
        &Gitignore::empty(),
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
        None,
        "color profile".to_owned(),
        None,
        Some(vec!["docs".to_owned()].into()),
        None,
        &Gitignore::empty(),
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
        None,
        "find me".to_owned(),
        None,
        Some(vec!["docs".to_owned()].into()),
        Some(vec!["md".to_owned()].into()),
        &Gitignore::empty(),
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
        None,
        "anything".to_owned(),
        None,
        Some(vec!["../escape".to_owned()].into()),
        None,
        &Gitignore::empty(),
    )
    .await;

    let err = result.expect_err("escape attempt must be a hard error");
    assert!(
        err.to_string().contains("escape the workspace"),
        "unexpected error: {err}"
    );
}

/// Workspace with one `.ignore`d fixture holding the sought text, mirroring
/// `**/fixtures/` in the real workspace `.ignore`.
fn fixtures_workspace(root: &camino::Utf8Path) {
    std::fs::write(root.join(".ignore"), "**/fixtures/\n").unwrap();
    std::fs::create_dir_all(root.join("crates/tests/fixtures")).unwrap();
    std::fs::write(
        root.join("crates/tests/fixtures/a.snap"),
        "context_window: None",
    )
    .unwrap();
}

#[tokio::test]
async fn ignored_directory_is_searched_when_named() {
    // Regression: a sweep scoped to an `.ignore`d directory searched nothing and
    // returned the same "no matches" as a real miss, which reads as evidence
    // that the pattern is absent from files the tool never opened.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fixtures_workspace(root);

    let matches = fs_grep_files(
        root,
        None,
        "context_window: None".to_owned(),
        None,
        Some(vec!["crates/tests/fixtures".to_owned()].into()),
        None,
        &Gitignore::empty(),
    )
    .await
    .unwrap()
    .replace('\\', "/");

    assert_eq!(
        matches,
        "crates/tests/fixtures/a.snap:1:context_window: None\n"
    );
}

#[tokio::test]
async fn explicitly_named_ignored_file_is_searched() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fixtures_workspace(root);

    let matches = fs_grep_files(
        root,
        None,
        "context_window".to_owned(),
        None,
        Some(vec!["crates/tests/fixtures/a.snap".to_owned()].into()),
        None,
        &Gitignore::empty(),
    )
    .await
    .unwrap()
    .replace('\\', "/");

    assert_eq!(
        matches,
        "crates/tests/fixtures/a.snap:1:context_window: None\n"
    );
}

#[tokio::test]
async fn suppressed_path_is_reported_so_the_caller_can_ask_the_user() {
    // The tool will not return this content however it is asked, so the note points
    // at the only route left rather than leaving the reader to conclude the text is
    // absent.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

    let matches = fs_grep_files(
        root,
        None,
        "refs/heads".to_owned(),
        None,
        Some(vec![".git/HEAD".to_owned()].into()),
        None,
        &suppress_matcher(root, &[".git/".to_owned()]).unwrap(),
    )
    .await
    .unwrap();

    // Normalized: a resolved path carries native separators, so the note reads
    // `.git\HEAD` on Windows.
    assert_eq!(
        matches.replace('\\', "/"),
        "No matches found in the paths that were searched.\n\nNote: '.git/HEAD' is suppressed \
         from this tool's results. If you need it, ask the user to provide it."
    );
}

/// Search a single-file workspace, returning the tool's rendered output.
///
/// The pattern semantics below are the contract the tool exposes to callers;
/// they must hold for whichever regex engine backs the search.
async fn grep_one_file(content: &[u8], pattern: &str) -> String {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("a.txt"), content).unwrap();

    fs_grep_files(
        root,
        None,
        pattern.to_owned(),
        None,
        None,
        None,
        &Gitignore::empty(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn caret_and_dollar_anchor_per_line() {
    let matches = grep_one_file(b"alpha\nbeta\nalphabet\n", "^alpha$").await;

    assert_eq!(matches, "a.txt:1:alpha\n");
}

#[tokio::test]
async fn dot_does_not_match_across_lines() {
    let matches = grep_one_file(b"a\nb\n", "a.b").await;

    assert_eq!(
        matches,
        "No matches found. Broaden your search to see more."
    );
}

#[tokio::test]
async fn word_boundaries_hold_and_matching_is_case_sensitive() {
    let matches = grep_one_file(b"foo\nfoobar\nFOO\n", r"\bfoo\b").await;

    assert_eq!(matches, "a.txt:1:foo\n");
}

#[tokio::test]
async fn repeated_match_on_one_line_prints_the_line_once() {
    let matches = grep_one_file(b"foo bar foo\n", "foo").await;

    assert_eq!(matches, "a.txt:1:foo bar foo\n");
}

#[tokio::test]
async fn a_zero_width_pattern_matches_an_empty_line() {
    let matches = grep_one_file(b"alpha\n\nbeta\n", "^$").await;

    assert_eq!(matches, "a.txt:2:\n");
}

#[tokio::test]
async fn a_line_that_is_not_utf8_does_not_abort_the_search() {
    // Latin-1 bytes in a text file are not binary (no NUL), so the search runs
    // on. Only the matched line reaches the output, which keeps the final
    // UTF-8 decode of the printer buffer intact.
    let matches = grep_one_file(b"caf\xe9 latte\nneedle here\n", "needle").await;

    assert_eq!(matches, "a.txt:2:needle here\n");
}

#[tokio::test]
async fn negative_lookahead_excludes_a_match() {
    let matches = grep_one_file(b"foo bar\nfoo baz\n", "foo (?!bar)").await;

    assert_eq!(matches, "a.txt:2:foo baz\n");
}

#[tokio::test]
async fn lookbehind_sees_text_before_the_match() {
    let matches = grep_one_file(b"let needle\nfn needle\n", "(?<=let )needle").await;

    assert_eq!(matches, "a.txt:1:let needle\n");
}

#[tokio::test]
async fn backreference_matches_a_repeated_group() {
    let matches = grep_one_file(b"hello hello\nhello world\n", r"(\w+) \1").await;

    assert_eq!(matches, "a.txt:1:hello hello\n");
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

        let matches = fs_grep_files(
            root,
            None,
            pattern.to_owned(),
            Some(5),
            paths,
            None,
            &Gitignore::empty(),
        )
        .await
        .unwrap()
        .replace('\\', "/");

        assert_eq!(matches, expected.join(""), "test case: {name}");
    }
}
