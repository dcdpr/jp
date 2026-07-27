use std::collections::HashMap;

use camino_tempfile::tempdir;
use jp_tool::{AccessPolicy, FsRule};

use super::{super::utils::suppress_matcher, *};

/// No suppression, for the tests that are not about it.
fn unsuppressed() -> Gitignore {
    Gitignore::empty()
}

#[tokio::test]
async fn restricted_policy_filters_listing_to_readable() {
    let ws = tempdir().unwrap();
    std::fs::create_dir(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(ws.path().join("secret.txt"), "").unwrap();

    // Only `src` is readable; a no-prefix listing must omit `secret.txt`.
    let policy = AccessPolicy {
        fs: vec![FsRule::new("src").with_read(true)],
        ..AccessPolicy::default()
    };
    let files: Vec<String> = fs_list_files(ws.path(), Some(&policy), None, None, &unsuppressed())
        .await
        .unwrap()
        .into_files()
        .into_iter()
        .map(|f| f.replace('\\', "/"))
        .collect();

    assert!(files.iter().any(|f| f == "src/lib.rs"), "got: {files:?}");
    assert!(
        !files.iter().any(|f| f == "secret.txt"),
        "ungranted file leaked: {files:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn lists_files_under_approved_external_mount() {
    use std::os::unix::fs::symlink;

    use jp_tool::{AccessPolicy, FsRule};

    let ws = tempdir().unwrap();
    let ext = tempdir().unwrap();
    let ext_canon = ext.path().canonicalize_utf8().unwrap();
    std::fs::write(ext_canon.join("a.rs"), "").unwrap();
    std::fs::create_dir(ext_canon.join("sub")).unwrap();
    std::fs::write(ext_canon.join("sub/b.rs"), "").unwrap();
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
    let files = fs_list_files(
        ws.path(),
        Some(&policy),
        Some(vec!["fork".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap()
    .into_files();

    assert!(files.iter().any(|f| f == "fork/a.rs"), "got: {files:?}");
    assert!(files.iter().any(|f| f == "fork/sub/b.rs"), "got: {files:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn listing_external_mount_without_grant_is_rejected() {
    use std::os::unix::fs::symlink;

    use jp_tool::{AccessPolicy, FsRule};

    let ws = tempdir().unwrap();
    let ext = tempdir().unwrap();
    std::fs::write(ext.path().canonicalize_utf8().unwrap().join("a.rs"), "").unwrap();
    symlink(ext.path(), ws.path().join("fork")).unwrap();

    // Policy grants workspace read but no external mount: the symlink escape is
    // rejected.
    let policy = AccessPolicy {
        fs: vec![FsRule::new("").with_read(true)],
        ..AccessPolicy::default()
    };
    let result = fs_list_files(
        ws.path(),
        Some(&policy),
        Some(vec!["fork".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await;

    assert!(result.is_err(), "expected escape rejection");
}

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

        let files = fs_list_files(root, None, prefixes, extensions, &unsuppressed())
            .await
            .unwrap();

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

    let files = fs_list_files(
        root,
        None,
        Some(vec![".".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap();

    let mut listed = files.into_files();
    listed.sort();
    assert_eq!(listed, vec!["a.txt".to_owned(), "b.txt".to_owned()]);
}

#[tokio::test]
async fn subdir_scope_respects_root_ignore() {
    // A workspace `.ignore` excludes a build-output dir nested two levels
    // below the scoped directory (mirroring `docs/.vitepress/dist/`). Scoping
    // the listing to the parent dir must still honor the exclusion: the walk
    // has to be anchored at the workspace root, where the anchored `.ignore`
    // pattern prunes reliably, not at the scoped subdirectory.
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(root.join(".ignore"), "docs/.vitepress/dist/\n").unwrap();

    for path in ["docs/getting-started.md", "docs/.vitepress/dist/index.html"] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    let files = fs_list_files(
        root,
        None,
        Some(vec!["docs".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap()
    .into_files()
    .into_iter()
    .map(|s| s.replace('\\', "/"))
    .collect::<Vec<_>>();

    assert_eq!(files, vec!["docs/getting-started.md".to_owned()]);
}

/// Workspace with one whitelisted source file and one `.ignore`d fixture tree,
/// mirroring `**/fixtures/` in the real workspace `.ignore`.
fn fixtures_workspace(root: &camino::Utf8Path) {
    std::fs::write(root.join(".ignore"), "**/fixtures/\n").unwrap();
    std::fs::create_dir_all(root.join("crates/tests/fixtures")).unwrap();
    std::fs::write(root.join("crates/tests/lib.rs"), "").unwrap();
    std::fs::write(root.join("crates/tests/fixtures/a.snap"), "").unwrap();
}

fn listed(files: Files) -> Vec<String> {
    files
        .into_files()
        .into_iter()
        .map(|f| f.replace('\\', "/"))
        .collect()
}

#[tokio::test]
async fn explicitly_named_ignored_file_is_listed() {
    // Ignore rules govern what traversal surfaces, not what a caller may name. A
    // path the caller already knows about is one it can already read, so
    // withholding it here would protect nothing.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fixtures_workspace(root);

    let files = fs_list_files(
        root,
        None,
        Some(vec!["crates/tests/fixtures/a.snap".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap();

    assert!(files.notes().is_empty(), "got: {:?}", files.notes());
    assert_eq!(listed(files), vec![
        "crates/tests/fixtures/a.snap".to_owned()
    ]);
}

#[tokio::test]
async fn ignored_directory_is_walked_when_named() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fixtures_workspace(root);

    let files = fs_list_files(
        root,
        None,
        Some(vec!["crates/tests/fixtures".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap();

    assert!(files.notes().is_empty(), "got: {:?}", files.notes());
    assert_eq!(listed(files), vec![
        "crates/tests/fixtures/a.snap".to_owned()
    ]);
}

#[tokio::test]
async fn nested_ignore_file_decides_how_a_named_directory_is_reached() {
    // A directory the rules exclude can only be reached by walking it as its own
    // root, and the rule here lives in `docs/.ignore` rather than at the
    // workspace root. Classifying from the root file alone would call this path
    // ordinary, scope the workspace walk to it with a filter, and return nothing
    // — because traversal prunes it on the way down.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("docs/dist")).unwrap();
    std::fs::write(root.join("docs/.ignore"), "dist/\n").unwrap();
    std::fs::write(root.join("docs/dist/index.html"), "").unwrap();

    let files = fs_list_files(
        root,
        None,
        Some(vec!["docs/dist".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap();

    assert_eq!(listed(files), vec!["docs/dist/index.html".to_owned()]);
}

#[tokio::test]
async fn ignored_tree_stays_out_of_an_unscoped_listing() {
    // Naming a tree reaches it; not naming one leaves it pruned. This is the
    // anti-bloat property the ignore rules exist for.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fixtures_workspace(root);

    let files = fs_list_files(root, None, None, None, &unsuppressed())
        .await
        .unwrap();

    assert_eq!(listed(files), vec![
        ".ignore".to_owned(),
        "crates/tests/lib.rs".to_owned(),
    ]);
}

#[tokio::test]
async fn path_outside_the_whitelist_is_walked_when_named() {
    // The real `.ignore` is a whitelist: `*` then un-ignores. A tree nobody has
    // gotten around to un-ignoring is invisible to an unscoped listing but still
    // reachable on request, so adding a directory does not make it unreadable
    // until someone updates `.ignore`.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join(".ignore"), "*\n!src/\n!src/**\n").unwrap();
    std::fs::create_dir_all(root.join("vendor")).unwrap();
    std::fs::write(root.join("vendor/dep.rs"), "").unwrap();

    let files = fs_list_files(
        root,
        None,
        Some(vec!["vendor".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap();

    assert_eq!(listed(files), vec!["vendor/dep.rs".to_owned()]);
}

#[tokio::test]
async fn suppressed_path_is_skipped_with_a_note() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "").unwrap();

    let files = fs_list_files(
        root,
        None,
        Some(vec![".git".to_owned()].into()),
        None,
        &suppress_matcher(root, &[".git/".to_owned()]),
    )
    .await
    .unwrap();

    assert_eq!(files.notes(), vec![
        "'.git' is suppressed from this tool's results. If you need it, ask the user to provide \
         it."
        .to_owned()
    ]);
    assert!(listed(files).is_empty());
}

#[tokio::test]
async fn suppress_pattern_covers_files_inside_the_named_directory() {
    // A pattern naming a directory covers the files under it, or suppression is
    // one path component away from being bypassed.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

    let files = fs_list_files(
        root,
        None,
        Some(vec![".git/HEAD".to_owned()].into()),
        None,
        &suppress_matcher(root, &[".git/".to_owned()]),
    )
    .await
    .unwrap();

    assert_eq!(files.notes(), vec![
        "'.git/HEAD' is suppressed from this tool's results. If you need it, ask the user to \
         provide it."
            .to_owned()
    ]);
    assert!(listed(files).is_empty());
}

#[tokio::test]
async fn suppress_patterns_match_at_any_depth() {
    // `.ignore` glob syntax, so one pattern covers a name wherever it appears —
    // including inside a nested tree that an anchored prefix would miss.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("crates/inner/target/debug")).unwrap();
    std::fs::write(root.join("crates/inner/target/debug/bin"), "").unwrap();

    let files = fs_list_files(
        root,
        None,
        Some(vec!["crates/inner/target".to_owned()].into()),
        None,
        &suppress_matcher(root, &["**/target/".to_owned()]),
    )
    .await
    .unwrap();

    assert_eq!(files.notes(), vec![
        "'crates/inner/target' is suppressed from this tool's results. If you need it, ask the \
         user to provide it."
            .to_owned()
    ]);
    assert!(listed(files).is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn an_in_workspace_symlink_cannot_dodge_suppression() {
    // Matching happens on the canonical form, so naming the link resolves to the
    // suppressed target instead of sliding past a pattern keyed on its name.
    use std::os::unix::fs::symlink;

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    symlink(".git", root.join("gitlink")).unwrap();

    let files = fs_list_files(
        root,
        None,
        Some(vec!["gitlink/HEAD".to_owned()].into()),
        None,
        &suppress_matcher(root, &[".git/".to_owned()]),
    )
    .await
    .unwrap();

    assert_eq!(files.notes(), vec![
        "'.git/HEAD' is suppressed from this tool's results. If you need it, ask the user to \
         provide it."
            .to_owned()
    ]);
    assert!(listed(files).is_empty());
}

#[tokio::test]
async fn suppressed_tree_is_pruned_from_traversal_without_an_ignore_entry() {
    // One `suppress` entry is enough on its own: there is no matching `.ignore`
    // entry to keep in sync.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("secrets")).unwrap();
    std::fs::write(root.join("secrets/key.pem"), "").unwrap();
    std::fs::write(root.join("main.rs"), "").unwrap();

    let files = fs_list_files(
        root,
        None,
        None,
        None,
        &suppress_matcher(root, &["secrets/".to_owned()]),
    )
    .await
    .unwrap();

    assert!(files.notes().is_empty(), "got: {:?}", files.notes());
    assert_eq!(listed(files), vec!["main.rs".to_owned()]);
}

#[cfg(unix)]
#[tokio::test]
async fn suppression_reaches_inside_an_approved_external_mount() {
    // The mount's contents live outside the workspace on disk, so pruning matches
    // the path as the caller sees it rather than its real location. An anchored
    // access rule could not express this at all.
    use std::os::unix::fs::symlink;

    let ws = tempdir().unwrap();
    let ext = tempdir().unwrap();
    let ext_canon = ext.path().canonicalize_utf8().unwrap();
    std::fs::create_dir(ext_canon.join(".git")).unwrap();
    std::fs::write(ext_canon.join(".git/HEAD"), "").unwrap();
    std::fs::write(ext_canon.join("a.rs"), "").unwrap();
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
    let files = fs_list_files(
        ws.path(),
        Some(&policy),
        Some(vec!["fork".to_owned()].into()),
        None,
        &suppress_matcher(ws.path(), &["**/.git/".to_owned()]),
    )
    .await
    .unwrap();

    assert_eq!(listed(files), vec!["fork/a.rs".to_owned()]);
}

#[tokio::test]
async fn suppressed_path_does_not_suppress_the_other_requested_paths() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "").unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();

    let files = fs_list_files(
        root,
        None,
        Some(vec![".git".to_owned(), "src".to_owned()].into()),
        None,
        &suppress_matcher(root, &[".git/".to_owned()]),
    )
    .await
    .unwrap();

    assert_eq!(files.notes(), vec![
        "'.git' is suppressed from this tool's results. If you need it, ask the user to provide \
         it."
        .to_owned()
    ]);
    assert_eq!(listed(files), vec!["src/lib.rs".to_owned()]);
}

#[tokio::test]
async fn explicitly_named_file_respects_read_policy() {
    // Naming a file outright skips the walk, so the walk's per-entry read check
    // never sees it. The grant is settled before the file is read, and an
    // ungranted one is reported rather than dropped.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();
    std::fs::write(root.join("secret.txt"), "").unwrap();

    let policy = AccessPolicy {
        fs: vec![FsRule::new("src").with_read(true)],
        ..AccessPolicy::default()
    };
    let files = fs_list_files(
        root,
        Some(&policy),
        Some(vec!["secret.txt".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap();

    assert_eq!(files.notes(), vec![
        "'secret.txt' is not readable by this tool and was skipped. If you need it, ask the user \
         to provide it."
            .to_owned()
    ]);
    assert!(listed(files).is_empty(), "ungranted file leaked");
}

#[tokio::test]
async fn explicitly_named_file_respects_extension_filter() {
    // `grep_user_docs` relies on the extension filter to keep non-prose out of
    // documentation searches; an explicit target must not escape it.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/config.mts"), "").unwrap();

    let files = fs_list_files(
        root,
        None,
        Some(vec!["docs/config.mts".to_owned()].into()),
        Some(vec!["md".to_owned()].into()),
        &unsuppressed(),
    )
    .await
    .unwrap();

    assert!(listed(files).is_empty());
}

#[tokio::test]
#[test_log::test]
async fn test_empty_list() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let files = fs_list_files(
        root,
        None,
        Some(vec!["foo".to_owned()].into()),
        None,
        &unsuppressed(),
    )
    .await
    .unwrap();

    // A prefix that names nothing is a filter that matches nothing. There is no
    // `.ignore` rule involved, so the result carries no note.
    assert!(files.notes().is_empty());
    assert!(files.into_files().is_empty());
}
