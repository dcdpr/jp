use camino::Utf8PathBuf;
use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn test_is_file_dirty_modified() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    // Second column 'M' indicates modified
    let runner = MockProcessRunner::success(" M test.rs\n");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    assert!(result);
}

#[test]
fn test_is_file_dirty_not_modified() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    // No output means no changes
    let runner = MockProcessRunner::success("");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    assert!(!result);
}

#[test]
fn test_is_file_dirty_not_a_git_repo() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    let runner = MockProcessRunner::error("fatal: not a git repository");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    // Should return false when not in a git repo
    assert!(!result);
}

mod entry_kind_helper {
    use super::*;

    #[test]
    fn missing_returns_none() {
        let dir = tempdir().unwrap();
        let kind = entry_kind(&dir.path().join("ghost.txt")).unwrap();
        assert_eq!(kind, None);
    }

    #[test]
    fn regular_file_returns_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "").unwrap();
        assert_eq!(entry_kind(&path).unwrap(), Some(EntryKind::File));
    }

    #[test]
    fn directory_returns_dir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub");
        std::fs::create_dir(&path).unwrap();
        assert_eq!(entry_kind(&path).unwrap(), Some(EntryKind::Dir));
    }

    #[cfg(unix)]
    #[test]
    fn live_symlink_returns_symlink_not_target_kind() {
        // The whole point: don't follow the link. Even when the target
        // exists and is a regular file, `entry_kind` reports the entry
        // itself — a symlink.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("real.txt"), "").unwrap();
        std::os::unix::fs::symlink(
            std::path::Path::new("real.txt"),
            dir.path().join("link.txt").as_std_path(),
        )
        .unwrap();

        assert_eq!(
            entry_kind(&dir.path().join("link.txt")).unwrap(),
            Some(EntryKind::Symlink)
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_returns_symlink_not_none() {
        // A dangling link still has an entry on disk. `is_file()` /
        // `exists()` would lie about this and return false; `entry_kind`
        // sees the link.
        let dir = tempdir().unwrap();
        std::os::unix::fs::symlink(
            std::path::Path::new("/tmp/jp-tools-entry-kind-dangling"),
            dir.path().join("broken").as_std_path(),
        )
        .unwrap();

        assert_eq!(
            entry_kind(&dir.path().join("broken")).unwrap(),
            Some(EntryKind::Symlink)
        );
    }
}

mod resolve_workspace_path {
    use super::*;

    #[test]
    fn rejects_absolute_path() {
        let dir = tempdir().unwrap();
        let err = resolve_workspace_path(dir.path(), "/etc/passwd", None).unwrap_err();
        assert!(err.contains("relative"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_escaping_parent_dir() {
        let dir = tempdir().unwrap();
        let err = resolve_workspace_path(dir.path(), "../../etc/passwd", None).unwrap_err();
        assert!(
            err.contains("escape the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_mid_path_parent_dirs_that_still_escape() {
        let dir = tempdir().unwrap();
        // Cleans to `../etc/passwd` — leading `..` survives normalization.
        let err = resolve_workspace_path(dir.path(), "foo/../../etc/passwd", None).unwrap_err();
        assert!(
            err.contains("escape the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn accepts_parent_dir_that_normalizes_within_workspace() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("target.rs"), "").unwrap();

        // `sub/../target.rs` cleans to `target.rs` — well within the workspace.
        let resolved = resolve_workspace_path(dir.path(), "sub/../target.rs", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("target.rs"));
    }

    #[test]
    fn accepts_parent_dir_mid_path_with_nested_target() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();

        // `a/b/../c.rs` cleans to `a/c.rs`.
        let resolved = resolve_workspace_path(dir.path(), "a/b/../c.rs", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("a/c.rs"));
    }

    #[test]
    fn rejects_empty_path() {
        let dir = tempdir().unwrap();
        let err = resolve_workspace_path(dir.path(), "", None).unwrap_err();
        assert!(err.contains("empty"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_oversized_component() {
        let dir = tempdir().unwrap();
        let long = "a".repeat(101);
        let err = resolve_workspace_path(dir.path(), &long, None).unwrap_err();
        assert!(
            err.contains("less than 100 characters"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_too_many_components() {
        let dir = tempdir().unwrap();
        let deep = vec!["a"; 21].join("/");
        let err = resolve_workspace_path(dir.path(), &deep, None).unwrap_err();
        assert!(
            err.contains("less than 20 components"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn accepts_normal_path_to_existing_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("foo.rs");
        std::fs::write(&file, "").unwrap();

        let resolved = resolve_workspace_path(dir.path(), "foo.rs", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("foo.rs"));
        // canonicalized absolute may differ from dir.path() if the temp dir
        // lives behind a symlink (e.g. /tmp -> /private/tmp on macOS), so we
        // just verify it resolves to the same file.
        assert!(resolved.absolute.exists());
        assert_eq!(
            resolved.absolute.canonicalize_utf8().unwrap(),
            file.canonicalize_utf8().unwrap()
        );
    }

    #[test]
    fn accepts_not_yet_existing_file_with_existing_parent() {
        let dir = tempdir().unwrap();

        let resolved = resolve_workspace_path(dir.path(), "new_file.rs", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("new_file.rs"));
        assert!(!resolved.absolute.exists());
        assert_eq!(resolved.absolute.file_name(), Some("new_file.rs"));
    }

    #[test]
    fn accepts_nested_path_with_partial_existing_parents() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        // dir/a exists; dir/a/b does not.

        let resolved = resolve_workspace_path(dir.path(), "a/b/c.rs", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("a/b/c.rs"));
    }

    #[test]
    fn accepts_current_dir_component() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("foo.rs");
        std::fs::write(&file, "").unwrap();

        let resolved = resolve_workspace_path(dir.path(), "./foo.rs", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("foo.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_symlink_at_final_position() {
        // Dangling symlinks at the final position let `canonicalize` return
        // NotFound while the entry itself still exists. Without the
        // `symlink_metadata` probe, the resolver would happily return the
        // link path and `fs_create_file` would then follow it with
        // `O_CREAT`, materializing the target outside the workspace.
        let workspace = tempdir().unwrap();
        // Point the symlink at something outside the workspace that does
        // not exist — the link is broken.
        std::os::unix::fs::symlink(
            std::path::Path::new("/tmp/does-not-exist-jp-tools-test"),
            workspace.path().join("dangling").as_std_path(),
        )
        .unwrap();

        let err = resolve_workspace_path(workspace.path(), "dangling", None).unwrap_err();
        assert!(
            err.contains("symlink with a missing or non-workspace target"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_symlink_as_parent_component() {
        let workspace = tempdir().unwrap();
        std::os::unix::fs::symlink(
            std::path::Path::new("/tmp/also-does-not-exist-jp-tools-test"),
            workspace.path().join("dangling").as_std_path(),
        )
        .unwrap();

        let err = resolve_workspace_path(workspace.path(), "dangling/child.rs", None).unwrap_err();
        assert!(
            err.contains("symlink with a missing or non-workspace target"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_for_existing_target() {
        let outside = tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "shhh").unwrap();

        let workspace = tempdir().unwrap();
        // workspace/linkfile is a symlink pointing at the outside file.
        std::os::unix::fs::symlink(
            secret.as_std_path(),
            workspace.path().join("linkfile").as_std_path(),
        )
        .unwrap();

        let err = resolve_workspace_path(workspace.path(), "linkfile", None).unwrap_err();
        assert!(
            err.contains("escapes the workspace"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_parent_directory_escape() {
        let outside = tempdir().unwrap();
        std::fs::create_dir(outside.path().join("real")).unwrap();

        let workspace = tempdir().unwrap();
        // workspace/linkdir is a symlink to an outside directory.
        std::os::unix::fs::symlink(
            outside.path().join("real").as_std_path(),
            workspace.path().join("linkdir").as_std_path(),
        )
        .unwrap();

        // Target file does not exist yet; the parent's canonicalization is what
        // catches the escape.
        let err = resolve_workspace_path(workspace.path(), "linkdir/new.rs", None).unwrap_err();
        assert!(
            err.contains("escapes the workspace"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn accepts_symlink_pointing_within_workspace() {
        let workspace = tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("real")).unwrap();
        std::fs::write(workspace.path().join("real/foo.rs"), "").unwrap();

        // workspace/link -> workspace/real
        std::os::unix::fs::symlink(
            workspace.path().join("real").as_std_path(),
            workspace.path().join("link").as_std_path(),
        )
        .unwrap();

        let resolved = resolve_workspace_path(workspace.path(), "link/foo.rs", None).unwrap();

        // The canonical relative reflects the real location, not the symlink.
        assert_eq!(resolved.relative, Utf8PathBuf::from("real/foo.rs"));
    }
}

mod resolve_workspace_entry {
    use super::*;

    // Shares input validation with `resolve_workspace_path`, so the
    // input-shape rejection tests are not duplicated here — just spot-check
    // a couple to make sure the wiring is in place.

    #[test]
    fn rejects_absolute_path() {
        let dir = tempdir().unwrap();
        let err = resolve_workspace_entry(dir.path(), "/etc/passwd", None).unwrap_err();
        assert!(err.contains("relative"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_escaping_parent_dir() {
        let dir = tempdir().unwrap();
        let err = resolve_workspace_entry(dir.path(), "../../etc/passwd", None).unwrap_err();
        assert!(
            err.contains("escape the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn accepts_normal_path_to_existing_file() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("foo.rs"), "").unwrap();

        let resolved = resolve_workspace_entry(dir.path(), "foo.rs", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("foo.rs"));
    }

    #[test]
    fn accepts_not_yet_existing_nested_target() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();

        // `a` exists; `a/b` does not. The parent walk should produce
        // `<canonical_root>/a/b/c.rs` with the suffix reattached after
        // canonicalization.
        let resolved = resolve_workspace_entry(dir.path(), "a/b/c.rs", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("a/b/c.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn preserves_symlink_at_final_position() {
        // The whole point of the entry resolver: do *not* follow a final
        // symlink. The returned absolute path must still point at the link
        // entry, not at its canonical target.
        let workspace = tempdir().unwrap();
        std::fs::write(workspace.path().join("real.txt"), "").unwrap();
        std::os::unix::fs::symlink(
            std::path::Path::new("real.txt"),
            workspace.path().join("link.txt").as_std_path(),
        )
        .unwrap();

        let resolved = resolve_workspace_entry(workspace.path(), "link.txt", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("link.txt"));
        // The entry is reachable via its symlink name, and is itself a
        // symlink — i.e. not canonicalized away.
        let meta = std::fs::symlink_metadata(&resolved.absolute).unwrap();
        assert!(meta.file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn accepts_dangling_symlink_at_final_position() {
        // For entry operations a broken final-position symlink is a
        // perfectly valid thing to name (delete it, rename it). The
        // resolver must not reject it the way `resolve_workspace_path`
        // does — only the *parent* canonicalization matters.
        let workspace = tempdir().unwrap();
        std::os::unix::fs::symlink(
            std::path::Path::new("/tmp/does-not-exist-jp-tools-entry"),
            workspace.path().join("broken").as_std_path(),
        )
        .unwrap();

        let resolved = resolve_workspace_entry(workspace.path(), "broken", None).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("broken"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_parent_escape() {
        // Parent canonicalization still has to land inside the workspace.
        let outside = tempdir().unwrap();
        std::fs::create_dir(outside.path().join("real")).unwrap();

        let workspace = tempdir().unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("real").as_std_path(),
            workspace.path().join("linkdir").as_std_path(),
        )
        .unwrap();

        let err = resolve_workspace_entry(workspace.path(), "linkdir/new.rs", None).unwrap_err();
        assert!(
            err.contains("escapes the workspace"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_symlink_as_parent_component() {
        // A broken parent symlink does mean we can't bound the target
        // anywhere — reject, the same way `resolve_workspace_path` does.
        let workspace = tempdir().unwrap();
        std::os::unix::fs::symlink(
            std::path::Path::new("/tmp/does-not-exist-jp-tools-entry-parent"),
            workspace.path().join("dangling").as_std_path(),
        )
        .unwrap();

        let err = resolve_workspace_entry(workspace.path(), "dangling/child.rs", None).unwrap_err();
        assert!(
            err.contains("symlink with a missing or non-workspace target"),
            "unexpected error: {err}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_drive_relative_input() {
        // `C:foo` on Windows is *drive-relative* — it has a Prefix
        // component but no root, so it slips past `is_absolute()` and
        // `has_root()`. The `Prefix(_)` arm in `validate_workspace_input`
        // is what stops it from reaching `root.join(...)`.
        let dir = tempdir().unwrap();
        let err = resolve_workspace_entry(dir.path(), "C:foo", None).unwrap_err();
        assert!(err.contains("relative"), "unexpected error: {err}");
    }
}

mod clean_workspace_path {
    use super::*;

    #[test]
    fn rejects_absolute_path() {
        let dir = tempdir().unwrap();
        let err = clean_workspace_path(dir.path(), "/etc/passwd", None).unwrap_err();
        assert!(err.contains("relative"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_escaping_parent_dir() {
        let dir = tempdir().unwrap();
        let err = clean_workspace_path(dir.path(), "../../etc/passwd", None).unwrap_err();
        assert!(
            err.contains("escape the workspace"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_empty_path() {
        let dir = tempdir().unwrap();
        let err = clean_workspace_path(dir.path(), "", None).unwrap_err();
        assert!(err.contains("empty"), "unexpected error: {err}");
    }

    #[test]
    fn accepts_normal_path_and_returns_cleaned_form() {
        let dir = tempdir().unwrap();
        let cleaned = clean_workspace_path(dir.path(), "src/main.rs", None).unwrap();
        assert_eq!(cleaned, Utf8PathBuf::from("src/main.rs"));
    }

    #[test]
    fn collapses_redundant_components() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let cleaned = clean_workspace_path(dir.path(), "sub/../target.rs", None).unwrap();
        assert_eq!(cleaned, Utf8PathBuf::from("target.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn preserves_symlink_input_shape() {
        // Where `resolve_workspace_path` would canonicalize the symlink and
        // return `real/foo.rs`, `clean_workspace_path` keeps the user's
        // input shape `link/foo.rs` — while still checking the escape.
        let workspace = tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("real")).unwrap();
        std::fs::write(workspace.path().join("real/foo.rs"), "").unwrap();
        std::os::unix::fs::symlink(
            workspace.path().join("real").as_std_path(),
            workspace.path().join("link").as_std_path(),
        )
        .unwrap();

        let cleaned = clean_workspace_path(workspace.path(), "link/foo.rs", None).unwrap();
        assert_eq!(cleaned, Utf8PathBuf::from("link/foo.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escaping_workspace() {
        let outside = tempdir().unwrap();
        std::fs::create_dir(outside.path().join("real")).unwrap();

        let workspace = tempdir().unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("real").as_std_path(),
            workspace.path().join("linkdir").as_std_path(),
        )
        .unwrap();

        let err = clean_workspace_path(workspace.path(), "linkdir/file.rs", None).unwrap_err();
        assert!(
            err.contains("escapes the workspace"),
            "unexpected error: {err}"
        );
    }
}
