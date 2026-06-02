use camino_tempfile::tempdir;
use jp_tool::Outcome;
use serde_json::Map;

use super::*;

fn no_answers() -> Map<String, serde_json::Value> {
    Map::new()
}

fn unwrap_success(o: Outcome) -> String {
    match o {
        Outcome::Success { content } => content,
        other => panic!("expected Success, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn external_mount_delete_respects_capability() {
    use std::os::unix::fs::symlink;

    use jp_tool::{AccessPolicy, FsRule};

    let workspace = tempdir().unwrap();
    let external = tempdir().unwrap();
    let external_canonical = external.path().canonicalize_utf8().unwrap();
    std::fs::write(external_canonical.join("f.txt"), "x").unwrap();
    // A second file keeps the directory non-empty so the delete doesn't trip
    // the empty-parent cleanup.
    std::fs::write(external_canonical.join("keep.txt"), "x").unwrap();
    symlink(external.path(), workspace.path().join("fork")).unwrap();

    let mount = |write: bool| AccessPolicy {
        fs: vec![{
            let rule = FsRule::new("fork")
                .with_external(true)
                .with_approved_target(Some(external_canonical.clone()))
                .with_read(true);
            if write { rule.with_write(true) } else { rule }
        }],
        ..AccessPolicy::default()
    };

    // Read-only mount: delete is denied and the file survives.
    let read_only = mount(false);
    let result = fs_delete_file(
        workspace.path(),
        Some(&read_only),
        &no_answers(),
        "fork/f.txt".to_owned(),
    )
    .await
    .unwrap();
    assert!(matches!(result, Outcome::Error { .. }));
    assert!(external_canonical.join("f.txt").exists());

    // Read-write mount: delete succeeds.
    let read_write = mount(true);
    let result = fs_delete_file(
        workspace.path(),
        Some(&read_write),
        &no_answers(),
        "fork/f.txt".to_owned(),
    )
    .await
    .unwrap();
    unwrap_success(result);
    assert!(!external_canonical.join("f.txt").exists());
}

#[tokio::test]
async fn deleting_last_root_level_file_preserves_workspace() {
    // Regression: when the deleted file was the only entry at the
    // workspace root, the empty-parent cleanup used to walk all the way up
    // to the workspace itself and try to `remove_dir` it. The relative-
    // parent guard skips the cleanup when the entry has no intermediate
    // parent, so the workspace survives.
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("only.txt"), "x").unwrap();

    let result = fs_delete_file(root, None, &no_answers(), "only.txt".to_owned())
        .await
        .unwrap();

    let msg = unwrap_success(result);
    assert!(
        msg.starts_with("File deleted."),
        "unexpected message: {msg}"
    );
    assert!(
        !msg.contains("Removed empty parent"),
        "must not attempt to remove the workspace root: {msg}"
    );
    assert!(
        root.exists() && root.is_dir(),
        "workspace root must still exist"
    );
    assert!(!root.join("only.txt").exists());
}

#[tokio::test]
async fn deleting_nested_file_removes_empty_parent() {
    // Regression: the cleanup should still fire for genuinely empty
    // intermediate parents, just not for the workspace root.
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("nested/inner")).unwrap();
    std::fs::write(root.join("nested/inner/file.txt"), "x").unwrap();

    let result = fs_delete_file(
        root,
        None,
        &no_answers(),
        "nested/inner/file.txt".to_owned(),
    )
    .await
    .unwrap();

    let msg = unwrap_success(result);
    assert!(
        msg.contains("Removed empty parent"),
        "expected parent-cleanup note in: {msg}"
    );
    assert!(!root.join("nested/inner").exists());
    // The cleanup only removes the immediate parent, not further ancestors.
    assert!(root.join("nested").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn deleting_symlink_removes_the_link_entry() {
    // `fs::remove_file` on a symlink unlinks the link itself, not the
    // target. With the entry resolver in place this is the expected
    // semantics: the user named the link, and that's what disappears.
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("real.txt"), "payload").unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new("real.txt"),
        root.join("link.txt").as_std_path(),
    )
    .unwrap();

    let result = fs_delete_file(root, None, &no_answers(), "link.txt".to_owned())
        .await
        .unwrap();

    unwrap_success(result);
    // Link entry is gone; target file is untouched.
    assert!(std::fs::symlink_metadata(root.join("link.txt")).is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("real.txt")).unwrap(),
        "payload"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn deleting_dangling_symlink_succeeds() {
    // A broken symlink is still a valid entry to delete. `is_file()` would
    // have reported "non-existing file" here pre-fix; `symlink_metadata`
    // correctly reports the link entry.
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::os::unix::fs::symlink(
        std::path::Path::new("/tmp/jp-tools-delete-dangling-test"),
        root.join("broken").as_std_path(),
    )
    .unwrap();

    let result = fs_delete_file(root, None, &no_answers(), "broken".to_owned())
        .await
        .unwrap();

    unwrap_success(result);
    assert!(std::fs::symlink_metadata(root.join("broken")).is_err());
}

#[tokio::test]
async fn deleting_missing_path_errors() {
    let dir = tempdir().unwrap();
    let result = fs_delete_file(dir.path(), None, &no_answers(), "ghost.txt".to_owned())
        .await
        .unwrap();

    match result {
        Outcome::Error { message, .. } => {
            assert!(
                message.contains("non-existing entry"),
                "unexpected error: {message}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn deleting_socket_entry_errors() {
    // Sockets (and FIFOs, block/char devices) map to `EntryKind::Other`.
    // `fs::remove_file` would happily unlink them, but the user-facing
    // tool is "delete a file" — surface the kind so the user can reach
    // for a different tool if they actually meant it.
    let dir = tempdir().unwrap();
    let root = dir.path();
    let socket_path = root.join("my.sock");
    // Bind keeps the file in place; tempdir cleanup will unlink it.
    let _listener = std::os::unix::net::UnixListener::bind(socket_path.as_std_path()).unwrap();

    let result = fs_delete_file(root, None, &no_answers(), "my.sock".to_owned())
        .await
        .unwrap();

    match result {
        Outcome::Error { message, .. } => {
            assert!(
                message.contains("regular file"),
                "unexpected error: {message}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
    // The socket entry should still be in place.
    let meta = std::fs::symlink_metadata(&socket_path).unwrap();
    assert!(
        !meta.file_type().is_file() && !meta.file_type().is_dir(),
        "socket entry should still exist as a non-file, non-dir"
    );
}

#[tokio::test]
async fn deleting_directory_errors() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("subdir")).unwrap();

    let result = fs_delete_file(root, None, &no_answers(), "subdir".to_owned())
        .await
        .unwrap();

    match result {
        Outcome::Error { message, .. } => {
            assert!(message.contains("directory"), "unexpected: {message}");
        }
        other => panic!("expected Error, got {other:?}"),
    }
    assert!(root.join("subdir").exists());
}
