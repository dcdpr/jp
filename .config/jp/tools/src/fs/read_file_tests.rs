use camino_tempfile::tempdir;
use jp_tool::{Action, Context, Outcome};

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
            expected: "```txt\n1: foo\n2: bar\n3: baz\n4: \n```\n".to_owned(),
        }),
        ("start line", TestCase {
            file_contents: "foo\nbar\nbaz\n".to_owned(),
            start_line: Some(2),
            end_line: None,
            expected: "```txt\n2: bar\n3: baz\n4: \n```\n".to_owned(),
        }),
        ("end line", TestCase {
            file_contents: "foo\nbar\nbaz\n".to_owned(),
            start_line: None,
            end_line: Some(2),
            expected: "```txt\n1: foo\n2: bar\n... (truncated after line #2) ...\n```\n".to_owned(),
        }),
        ("start and end line", TestCase {
            file_contents: "foo\nbar\nbaz\n\n".to_owned(),
            start_line: Some(2),
            end_line: Some(2),
            expected: "```txt\n2: bar\n... (truncated after line #2) ...\n```\n".to_owned(),
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

        let ctx = Context {
            root: tmp.path().to_path_buf(),
            action: Action::Run,
            access: None,
        };
        let result = fs_read_file(&ctx, "file.txt".to_owned(), start_line, end_line)
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

#[cfg(unix)]
#[tokio::test]
async fn reads_through_approved_external_mount() {
    use std::os::unix::fs::symlink;

    use jp_tool::{AccessPolicy, FsRule};

    let workspace = tempdir().unwrap();
    let external = tempdir().unwrap();
    let external_canonical = external.path().canonicalize_utf8().unwrap();
    std::fs::write(external_canonical.join("lib.rs"), "external contents\n").unwrap();

    // <ws>/fork -> <external>
    symlink(external.path(), workspace.path().join("fork")).unwrap();

    let ctx = Context {
        root: workspace.path().to_path_buf(),
        action: Action::Run,
        access: Some(AccessPolicy {
            fs: vec![
                FsRule::new("fork")
                    .with_external(true)
                    .with_approved_target(Some(external_canonical))
                    .with_read(true),
            ],
            ..AccessPolicy::default()
        }),
    };

    let result = fs_read_file(&ctx, "fork/lib.rs".to_owned(), None, None)
        .await
        .unwrap();

    let content = match result {
        Outcome::Success { content } => content,
        other => panic!("expected success, got {other:?}"),
    };
    assert!(
        content.contains("external contents"),
        "should read the external file through the mount: {content}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn read_through_internal_symlink_respects_deny_rule() {
    use std::os::unix::fs::symlink;

    use jp_tool::{AccessPolicy, FsRule};

    let workspace = tempdir().unwrap();
    let root = workspace.path().canonicalize_utf8().unwrap();
    std::fs::create_dir(root.join("secret")).unwrap();
    std::fs::write(root.join("secret/f.txt"), "classified").unwrap();
    // An in-workspace symlink to the denied directory.
    symlink(root.join("secret"), root.join("alias")).unwrap();

    let ctx = Context {
        root: root.clone(),
        action: Action::Run,
        access: Some(AccessPolicy {
            fs: vec![
                FsRule::new("").with_read(true),
                FsRule::new("secret").with_read(false),
            ],
            ..AccessPolicy::default()
        }),
    };

    // Direct read of the denied path is rejected.
    let direct = fs_read_file(&ctx, "secret/f.txt".to_owned(), None, None)
        .await
        .unwrap();
    assert!(
        matches!(direct, Outcome::Error { .. }),
        "direct read should be denied"
    );

    // Reaching it through the in-workspace symlink canonicalizes to `secret/`
    // and must be denied too — the symlink cannot dodge the deny rule.
    let via_alias = fs_read_file(&ctx, "alias/f.txt".to_owned(), None, None)
        .await
        .unwrap();
    assert!(
        matches!(via_alias, Outcome::Error { .. }),
        "symlinked read should be denied"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn denies_in_workspace_path_with_no_matching_grant() {
    use jp_tool::{AccessPolicy, FsRule};

    let workspace = tempdir().unwrap();
    std::fs::write(workspace.path().join("secret.txt"), "nope").unwrap();

    // A policy that only grants an external mount; in-workspace reads with no
    // matching rule are default-denied.
    let ctx = Context {
        root: workspace.path().to_path_buf(),
        action: Action::Run,
        access: Some(AccessPolicy {
            fs: vec![FsRule::new("fork").with_external(true).with_read(true)],
            ..AccessPolicy::default()
        }),
    };

    let result = fs_read_file(&ctx, "secret.txt".to_owned(), None, None)
        .await
        .unwrap();
    assert!(matches!(result, Outcome::Error { .. }));
}
