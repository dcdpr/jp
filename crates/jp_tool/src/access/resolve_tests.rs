use camino_tempfile::{Utf8TempDir, tempdir};

use super::*;
use crate::FsRule;

/// A workspace, and its root canonicalized as the resolver reports it.
fn workspace() -> (Utf8TempDir, Utf8PathBuf) {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize_utf8().unwrap();
    (dir, root)
}

fn resolve(root: &Utf8Path, input: &str, mode: Resolve) -> Result<ResolvedPath, FsAccessError> {
    resolve_path(root, Utf8Path::new(input), None, mode)
}

mod entry_kind {
    use super::*;

    #[test]
    fn missing_is_none() {
        let (_dir, root) = workspace();
        assert_eq!(super::entry_kind(&root.join("ghost.txt")).unwrap(), None);
    }

    #[test]
    fn file_and_directory() {
        let (_dir, root) = workspace();
        std::fs::write(root.join("a.txt"), "").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();

        assert_eq!(
            super::entry_kind(&root.join("a.txt")).unwrap(),
            Some(EntryKind::File)
        );
        assert_eq!(
            super::entry_kind(&root.join("sub")).unwrap(),
            Some(EntryKind::Dir)
        );
    }

    /// The link is reported, not what it points at, live or dangling.
    #[cfg(unix)]
    #[test]
    fn symlink_live_or_dangling() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = workspace();
        std::fs::write(root.join("real.txt"), "").unwrap();
        symlink("real.txt", root.join("live")).unwrap();
        symlink("missing.txt", root.join("dangling")).unwrap();

        for name in ["live", "dangling"] {
            assert_eq!(
                super::entry_kind(&root.join(name)).unwrap(),
                Some(EntryKind::Symlink),
                "entry: {name}"
            );
        }
    }
}

mod input {
    use super::*;

    #[test]
    fn absolute_is_rejected() {
        let (_dir, root) = workspace();
        assert_eq!(
            resolve(&root, "/etc/passwd", Resolve::Target),
            Err(FsAccessError::Absolute("/etc/passwd".into()))
        );
    }

    #[test]
    fn escaping_parent_dirs_are_rejected() {
        let (_dir, root) = workspace();
        for input in ["../../etc/passwd", "foo/../../etc/passwd"] {
            assert_eq!(
                resolve(&root, input, Resolve::Target),
                Err(FsAccessError::InputEscape(input.into())),
                "input: {input}"
            );
        }
    }

    #[test]
    fn parent_dirs_that_stay_inside_are_normalized() {
        let (_dir, root) = workspace();
        std::fs::create_dir_all(root.join("a/b")).unwrap();

        let resolved = resolve(&root, "./a/b/../c.rs", Resolve::Target).unwrap();

        assert_eq!(resolved, ResolvedPath {
            absolute: root.join("a/c.rs"),
            relative: "a/c.rs".into(),
            lexical: "a/c.rs".into(),
            kind: None,
            holds_workspace: false,
        });
    }

    #[test]
    fn empty_is_the_root() {
        let (_dir, root) = workspace();

        let resolved = resolve(&root, "", Resolve::Entry).unwrap();

        assert_eq!(resolved.absolute, root);
        assert_eq!(resolved.relative, Utf8PathBuf::new());
        assert!(resolved.is_dir());
    }

    /// `C:foo` is drive-relative: it has a drive prefix but no root, so it
    /// slips past `is_absolute` and `has_root`.
    #[cfg(windows)]
    #[test]
    fn windows_drive_relative_is_rejected() {
        let (_dir, root) = workspace();
        assert_eq!(
            resolve(&root, "C:foo", Resolve::Entry),
            Err(FsAccessError::Absolute("C:foo".into()))
        );
    }
}

mod target {
    use super::*;

    #[test]
    fn existing_file() {
        let (_dir, root) = workspace();
        std::fs::write(root.join("foo.rs"), "").unwrap();

        let resolved = resolve(&root, "foo.rs", Resolve::Target).unwrap();

        assert_eq!(resolved.absolute, root.join("foo.rs"));
        assert_eq!(resolved.relative, Utf8PathBuf::from("foo.rs"));
        assert_eq!(resolved.kind, Some(EntryKind::File));
    }

    #[test]
    fn missing_components_are_appended_to_the_deepest_existing_ancestor() {
        let (_dir, root) = workspace();
        std::fs::create_dir(root.join("a")).unwrap();

        let resolved = resolve(&root, "a/b/c.rs", Resolve::Target).unwrap();

        assert_eq!(resolved.absolute, root.join("a/b/c.rs"));
        assert_eq!(resolved.relative, Utf8PathBuf::from("a/b/c.rs"));
        assert_eq!(resolved.kind, None);
    }

    /// Rules match the real location, so a link cannot dodge a rule on it.
    #[cfg(unix)]
    #[test]
    fn symlink_inside_the_workspace_resolves_to_its_real_location() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = workspace();
        std::fs::create_dir(root.join("real")).unwrap();
        std::fs::write(root.join("real/foo.rs"), "").unwrap();
        symlink(root.join("real"), root.join("link")).unwrap();

        let resolved = resolve(&root, "link/foo.rs", Resolve::Target).unwrap();

        assert_eq!(resolved.absolute, root.join("real/foo.rs"));
        assert_eq!(resolved.relative, Utf8PathBuf::from("real/foo.rs"));
        assert_eq!(resolved.lexical, Utf8PathBuf::from("link/foo.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_outside_escapes() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = workspace();
        let (_outside, outside) = workspace();
        std::fs::write(outside.join("secret.txt"), "").unwrap();
        std::fs::create_dir(outside.join("real")).unwrap();
        symlink(outside.join("secret.txt"), root.join("linkfile")).unwrap();
        symlink(outside.join("real"), root.join("linkdir")).unwrap();

        assert_eq!(
            resolve(&root, "linkfile", Resolve::Target),
            Err(FsAccessError::Escape(outside.join("secret.txt")))
        );
        // A target that does not exist yet escapes through its parent.
        assert_eq!(
            resolve(&root, "linkdir/new.rs", Resolve::Target),
            Err(FsAccessError::Escape(outside.join("real/new.rs")))
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_rejected_at_any_position() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = workspace();
        let (_outside, outside) = workspace();
        symlink(outside.join("missing"), root.join("dangling")).unwrap();

        for input in ["dangling", "dangling/child.rs"] {
            assert_eq!(
                resolve(&root, input, Resolve::Target),
                Err(FsAccessError::DanglingSymlink("dangling".into())),
                "input: {input}"
            );
        }
    }

    /// A file where a directory is expected fails rather than resolving to a
    /// path nothing can be created at.
    #[test]
    fn a_file_as_a_parent_is_an_io_error() {
        let (_dir, root) = workspace();
        std::fs::write(root.join("file.txt"), "").unwrap();

        assert!(matches!(
            resolve(&root, "file.txt/child", Resolve::Target),
            Err(FsAccessError::Io { .. })
        ));
    }
}

/// Every case here involves a symlink, so the module is Unix-only.
#[cfg(unix)]
mod entry {
    use super::*;

    /// Deleting or renaming the entry acts on the link, not its target.
    #[test]
    fn a_final_symlink_is_left_as_named() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = workspace();
        std::fs::write(root.join("real.txt"), "").unwrap();
        symlink("real.txt", root.join("link.txt")).unwrap();

        let resolved = resolve(&root, "link.txt", Resolve::Entry).unwrap();

        assert_eq!(resolved, ResolvedPath {
            absolute: root.join("link.txt"),
            relative: "link.txt".into(),
            lexical: "link.txt".into(),
            kind: Some(EntryKind::Symlink),
            holds_workspace: false,
        });
    }

    /// A broken link is an entry like any other: it can be deleted or renamed.
    #[test]
    fn a_final_dangling_symlink_is_accepted() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = workspace();
        let (_outside, outside) = workspace();
        symlink(outside.join("missing"), root.join("broken")).unwrap();

        let resolved = resolve(&root, "broken", Resolve::Entry).unwrap();

        assert_eq!(resolved.relative, Utf8PathBuf::from("broken"));
        assert_eq!(resolved.kind, Some(EntryKind::Symlink));
    }

    #[test]
    fn the_parent_must_stay_inside() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = workspace();
        let (_outside, outside) = workspace();
        std::fs::create_dir(outside.join("real")).unwrap();
        symlink(outside.join("real"), root.join("linkdir")).unwrap();
        symlink(outside.join("missing"), root.join("dangling")).unwrap();

        assert_eq!(
            resolve(&root, "linkdir/new.rs", Resolve::Entry),
            Err(FsAccessError::Escape(outside.join("real/new.rs")))
        );
        assert_eq!(
            resolve(&root, "dangling/child.rs", Resolve::Entry),
            Err(FsAccessError::DanglingSymlink("dangling".into()))
        );
    }

    /// The mount link itself is a workspace entry; only a path through it
    /// reaches back in.
    #[test]
    fn a_mount_link_is_an_entry_not_a_way_back_in() {
        use std::os::unix::fs::symlink;

        let (_dir, grand) = workspace();
        let root = grand.join("ws");
        std::fs::create_dir(&root).unwrap();
        symlink(&grand, root.join("up")).unwrap();
        let mount = FsRule::new("up")
            .with_external(true)
            .with_approved_target(Some(grand.clone()))
            .with_read(true);
        let policy = AccessPolicy {
            fs: vec![mount],
            ..AccessPolicy::default()
        };

        let link = resolve_path(&root, "up".into(), Some(&policy), Resolve::Entry).unwrap();
        assert_eq!(link.kind, Some(EntryKind::Symlink));
        assert_eq!(link.relative, Utf8PathBuf::from("up"));

        assert_eq!(
            resolve_path(&root, "up/ws".into(), Some(&policy), Resolve::Entry),
            Err(FsAccessError::ThroughMount {
                path: "up/ws".into(),
                mount: "up".into(),
                workspace_path: ".".into(),
            })
        );
    }
}

/// Without an approved mount, a link out of the workspace is an escape even
/// when the entry it names is the workspace itself.
#[cfg(unix)]
#[test]
fn an_entry_named_through_a_plain_link_to_the_parent_escapes() {
    use std::os::unix::fs::symlink;

    let (_dir, grand) = workspace();
    let root = grand.join("ws");
    std::fs::create_dir(&root).unwrap();
    symlink(&grand, root.join("link")).unwrap();

    assert_eq!(
        resolve(&root, "link/ws", Resolve::Entry),
        Err(FsAccessError::Escape(root.clone()))
    );
}

mod authorize {
    use super::*;

    /// The refusal lists where the capability is granted, so the caller has
    /// somewhere to go instead.
    #[test]
    fn denial_lists_the_paths_granting_the_capability() {
        let (_dir, root) = workspace();
        std::fs::create_dir(root.join("src")).unwrap();
        let policy = AccessPolicy {
            fs: vec![
                FsRule::new("").with_read(true),
                FsRule::new("build").with_write(true),
                FsRule::new("docs").with_read(true).with_write(true),
            ],
            ..AccessPolicy::default()
        };
        let target = resolve_path(&root, "src".into(), Some(&policy), Resolve::Entry).unwrap();

        assert_eq!(
            policy.authorize(Capability::Delete, &target),
            Err(FsAccessError::Denied {
                capability: "delete",
                target: "src".into(),
                grants: vec!["build".into(), "docs".into()],
            })
        );
        assert_eq!(policy.authorize(Capability::Read, &target), Ok(()));
    }
}
