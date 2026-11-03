//! Integration tests for filesystem access checks, including the external
//! symlink resolution and nested-escape boundary introduced by RFD D43.

use camino::Utf8PathBuf;
use jp_tool::{AccessPolicy, Action, Capability, Context, FsAccessError, FsRule};

fn ctx(root: Utf8PathBuf, access: Option<AccessPolicy>) -> Context {
    Context {
        root,
        action: Action::Run,
        access,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    }
}

#[test]
fn unrestricted_allows_inside_rejects_outside_shape() {
    let dir = camino_tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    std::fs::write(root.join("file.txt"), "x").unwrap();

    let ctx = ctx(root, None);

    // Inside the workspace: allowed, returns the resolved path.
    assert!(ctx.check_read("file.txt".into()).is_ok());

    // Absolute input is rejected before any I/O.
    assert!(matches!(
        ctx.check_read("/etc/passwd".into()),
        Err(FsAccessError::Absolute(_))
    ));

    // A `..`-escape in the input is rejected before any I/O.
    assert!(matches!(
        ctx.check_read("../outside".into()),
        Err(FsAccessError::InputEscape(_))
    ));
}

#[test]
fn permits_enforces_capabilities_and_default_deny() {
    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("src").with_read(true),
            FsRule::new("build").with_read(true).with_write(true),
        ],
        ..AccessPolicy::default()
    };

    // `src` is read-only.
    assert!(policy.permits(Capability::Read, "src/lib.rs".into()));
    assert!(!policy.permits(Capability::Update, "src/lib.rs".into()));
    assert!(!policy.permits(Capability::Delete, "src/lib.rs".into()));

    // `build` grants the write alias (create/update/delete).
    assert!(policy.permits(Capability::Create, "build/out".into()));
    assert!(policy.permits(Capability::Delete, "build/out".into()));

    // Unmatched paths are default-denied.
    assert!(!policy.permits(Capability::Read, "other/x".into()));

    // An unrestricted (empty) policy permits everything.
    assert!(AccessPolicy::default().permits(Capability::Delete, "anything".into()));
}

#[test]
fn restricted_default_deny_and_capabilities() {
    let dir = camino_tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "x").unwrap();
    std::fs::write(root.join("secret.txt"), "x").unwrap();

    let policy = AccessPolicy {
        fs: vec![FsRule::new("src").with_read(true)],
        ..AccessPolicy::default()
    };
    let ctx = ctx(root, Some(policy));

    // Granted read under `src`.
    assert!(ctx.check_read("src/lib.rs".into()).is_ok());
    // Write is not granted.
    assert!(matches!(
        ctx.check_update("src/lib.rs".into()),
        Err(FsAccessError::Denied { .. })
    ));
    // A path with no matching rule is default-denied.
    assert!(matches!(
        ctx.check_read("secret.txt".into()),
        Err(FsAccessError::Denied { .. })
    ));
}

#[cfg(unix)]
#[test]
fn internal_symlink_cannot_bypass_specific_deny() {
    use std::os::unix::fs::symlink;

    let dir = camino_tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize_utf8().unwrap();
    std::fs::create_dir(root.join("secret")).unwrap();
    std::fs::write(root.join("secret/f.txt"), "x").unwrap();
    // An in-workspace symlink to the denied directory.
    symlink(root.join("secret"), root.join("alias")).unwrap();

    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("").with_read(true),
            FsRule::new("secret").with_read(false),
        ],
        ..AccessPolicy::default()
    };
    let ctx = ctx(root, Some(policy));

    // Direct access to the denied directory is rejected.
    assert!(matches!(
        ctx.check_read("secret/f.txt".into()),
        Err(FsAccessError::Denied { .. })
    ));
    // Access via the in-workspace symlink canonicalizes to `secret/` and is
    // rejected too — the symlink cannot dodge the more specific deny rule.
    assert!(matches!(
        ctx.check_read("alias/f.txt".into()),
        Err(FsAccessError::Denied { .. })
    ));
}

#[cfg(unix)]
#[test]
fn internal_symlink_inherits_target_grant() {
    use std::os::unix::fs::symlink;

    let dir = camino_tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize_utf8().unwrap();
    std::fs::create_dir(root.join("real")).unwrap();
    std::fs::write(root.join("real/f.txt"), "x").unwrap();
    symlink(root.join("real"), root.join("alias")).unwrap();

    // Only `real` is granted; `alias` has no rule of its own.
    let policy = AccessPolicy {
        fs: vec![FsRule::new("real").with_read(true)],
        ..AccessPolicy::default()
    };
    let ctx = ctx(root, Some(policy));

    // Reaching the granted target through the symlink is allowed because it
    // canonicalizes to `real/`.
    assert!(ctx.check_read("alias/f.txt".into()).is_ok());
}

#[cfg(unix)]
#[test]
fn external_rule_permits_resolution_within_approved_target() {
    use std::os::unix::fs::symlink;

    let workspace = camino_tempfile::tempdir().unwrap();
    let external = camino_tempfile::tempdir().unwrap();
    let root = workspace.path().to_owned();

    let external_canonical = external.path().canonicalize_utf8().unwrap();
    std::fs::write(external_canonical.join("lib.rs"), "x").unwrap();

    // <ws>/fork -> <external>
    symlink(external.path(), root.join("fork")).unwrap();

    let policy = AccessPolicy {
        fs: vec![
            FsRule::new(".").with_read(true),
            FsRule::new("fork")
                .with_external(true)
                .with_approved_target(Some(external_canonical.clone()))
                .with_read(true)
                .with_write(true),
        ],
        ..AccessPolicy::default()
    };
    let ctx = ctx(root, Some(policy));

    let resolved = ctx.check_read("fork/lib.rs".into()).unwrap();
    assert!(resolved.starts_with(&external_canonical));

    // Write capability granted on the external rule.
    assert!(ctx.check_update("fork/lib.rs".into()).is_ok());
}

/// A mount whose target contains the workspace, as `--mount up=../..` from
/// `<grand>/a/ws` creates.
#[cfg(unix)]
fn ancestor_mount() -> (camino_tempfile::Utf8TempDir, Context) {
    use std::os::unix::fs::symlink;

    let grand = camino_tempfile::tempdir().unwrap();
    let grand_canonical = grand.path().canonicalize_utf8().unwrap();
    let root = grand_canonical.join("a/ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(grand_canonical.join("sibling")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "x").unwrap();
    std::fs::write(grand_canonical.join("sibling/f.txt"), "x").unwrap();
    // An alias for a directory holding the workspace, as a `current ->
    // releases/vN` checkout has.
    symlink(grand_canonical.join("a"), grand_canonical.join("current")).unwrap();
    symlink(&grand_canonical, root.join("up")).unwrap();

    let mount = FsRule::new("up")
        .with_external(true)
        .with_approved_target(Some(grand_canonical))
        .with_read(true)
        .with_write(true);
    let exclusion = mount.workspace_exclusion(&root).unwrap();
    assert_eq!(exclusion, FsRule::new("up/a/ws"));

    // The workspace itself is fully read-write, so nothing below is refused
    // for lack of a workspace grant.
    let policy = AccessPolicy {
        fs: vec![
            FsRule::new(".").with_read(true).with_write(true),
            mount,
            exclusion,
        ],
        ..AccessPolicy::default()
    };

    (grand, ctx(root, Some(policy)))
}

#[cfg(unix)]
#[test]
fn a_mount_containing_the_workspace_still_reaches_what_is_beside_it() {
    let (_grand, ctx) = ancestor_mount();

    assert!(ctx.check_update("up/sibling/f.txt".into()).is_ok());
    // A file in a directory on the way down to the workspace.
    assert!(ctx.check_create("up/a/new.txt".into()).is_ok());
    // The workspace's own paths keep the workspace's own rules.
    assert!(ctx.check_update("src/lib.rs".into()).is_ok());
}

/// Moving or deleting a directory on the way down to the workspace takes the
/// workspace along, so the mount must not grant it, under any name.
#[cfg(unix)]
#[test]
fn a_mount_cannot_move_the_workspace_or_a_directory_holding_it() {
    let (_grand, ctx) = ancestor_mount();

    for path in ["up", "up/a", "up/current"] {
        assert_eq!(
            ctx.check_delete(path.into()),
            Err(FsAccessError::HoldsWorkspace {
                capability: "delete",
                path: path.into(),
            }),
            "path: {path}"
        );
    }
}

/// A symlink to a directory holding the workspace is only a link: removing or
/// renaming it leaves the directory where it is.
#[cfg(unix)]
#[test]
fn a_mount_can_remove_a_link_to_a_directory_holding_the_workspace() {
    use jp_tool::{EntryKind, Resolve};

    let (_grand, ctx) = ancestor_mount();

    let link = ctx.resolve("up/current".into(), Resolve::Entry).unwrap();

    assert_eq!(link.kind, Some(EntryKind::Symlink));
    assert_eq!(ctx.authorize(Capability::Delete, &link), Ok(()));
}

/// A workspace file has one path: going through the mount to reach it is
/// refused, and the error names the path that works.
#[cfg(unix)]
#[test]
fn a_mount_does_not_reach_back_into_the_workspace() {
    let (_grand, ctx) = ancestor_mount();

    assert_eq!(
        ctx.check_read("up/a/ws/src/lib.rs".into()),
        Err(FsAccessError::ThroughMount {
            path: "up/a/ws/src/lib.rs".into(),
            mount: "up".into(),
            workspace_path: "src/lib.rs".into(),
        })
    );
    assert_eq!(
        ctx.check_delete("up/a/ws".into()),
        Err(FsAccessError::ThroughMount {
            path: "up/a/ws".into(),
            mount: "up".into(),
            workspace_path: ".".into(),
        })
    );
}

#[cfg(unix)]
#[test]
fn external_rule_blocks_nested_escape() {
    use std::os::unix::fs::symlink;

    let workspace = camino_tempfile::tempdir().unwrap();
    let external = camino_tempfile::tempdir().unwrap();
    let evil = camino_tempfile::tempdir().unwrap();
    let root = workspace.path().to_owned();

    let external_canonical = external.path().canonicalize_utf8().unwrap();
    let evil_canonical = evil.path().canonicalize_utf8().unwrap();
    std::fs::write(evil_canonical.join("passwd"), "secret").unwrap();

    // <ws>/fork -> <external>, and <external>/secrets -> <evil>
    symlink(external.path(), root.join("fork")).unwrap();
    symlink(evil.path(), external_canonical.join("secrets")).unwrap();

    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("fork")
                .with_external(true)
                .with_approved_target(Some(external_canonical))
                .with_read(true),
        ],
        ..AccessPolicy::default()
    };
    let ctx = ctx(root, Some(policy));

    // The nested symlink resolves outside the approved target → reject.
    assert!(matches!(
        ctx.check_read("fork/secrets/passwd".into()),
        Err(FsAccessError::Escape(_))
    ));
}

/// A dangling symlink has no target to bound, and creating a file through it
/// would land wherever it points.
#[cfg(unix)]
#[test]
fn dangling_symlink_pointing_outside_is_rejected() {
    use std::os::unix::fs::symlink;

    let workspace = camino_tempfile::tempdir().unwrap();
    let outside = camino_tempfile::tempdir().unwrap();
    let root = workspace.path().canonicalize_utf8().unwrap();
    let missing = outside.path().canonicalize_utf8().unwrap().join("missing");
    symlink(&missing, root.join("link")).unwrap();

    let ctx = ctx(root, None);

    for path in ["link", "link/child.rs"] {
        assert_eq!(
            ctx.check_create(path.into()),
            Err(FsAccessError::DanglingSymlink("link".into())),
            "path: {path}"
        );
    }
    assert!(!missing.exists());
}

#[cfg(unix)]
#[test]
fn external_rule_without_approval_is_rejected() {
    use std::os::unix::fs::symlink;

    let workspace = camino_tempfile::tempdir().unwrap();
    let external = camino_tempfile::tempdir().unwrap();
    let root = workspace.path().to_owned();

    let external_canonical = external.path().canonicalize_utf8().unwrap();
    std::fs::write(external_canonical.join("lib.rs"), "x").unwrap();
    symlink(external.path(), root.join("fork")).unwrap();

    // External rule but no approved target: resolution must be rejected.
    let policy = AccessPolicy {
        fs: vec![FsRule::new("fork").with_external(true).with_read(true)],
        ..AccessPolicy::default()
    };
    let ctx = ctx(root, Some(policy));

    assert!(matches!(
        ctx.check_read("fork/lib.rs".into()),
        Err(FsAccessError::Escape(_))
    ));
}

#[cfg(unix)]
#[test]
fn non_external_symlink_to_outside_is_escape() {
    use std::os::unix::fs::symlink;

    let workspace = camino_tempfile::tempdir().unwrap();
    let external = camino_tempfile::tempdir().unwrap();
    let root = workspace.path().to_owned();
    std::fs::write(external.path().canonicalize_utf8().unwrap().join("f"), "x").unwrap();
    symlink(external.path(), root.join("link")).unwrap();

    // Unrestricted, but the resolved target escapes the workspace.
    let ctx = ctx(root, None);
    assert!(matches!(
        ctx.check_read("link/f".into()),
        Err(FsAccessError::Escape(_))
    ));
}
