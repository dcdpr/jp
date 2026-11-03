use camino_tempfile::Utf8TempDir;
use jp_tool::FsRule;

use super::*;

#[test]
fn a_command_the_user_runs_reaches_everything() {
    let access = Access::Unrestricted;

    assert!(access.reads(Utf8Path::new("/anywhere/at/all.md")));
    assert!(access.allows(Capability::Delete, Utf8Path::new("/w/docs/wiki")));
    assert!(access.reads_workspace());
    assert!(!access.is_restricted());
}

#[test]
fn a_tool_call_reaches_what_its_policy_grants() {
    let root = Utf8TempDir::new().unwrap();
    let access = Access::granting(root.path(), vec![
        FsRule::new("docs/wiki").with_read(true).with_update(true),
    ]);

    assert!(access.reads(&root.path().join("docs/wiki/0a1b2c3-glossary.md")));
    assert!(access.allows(
        Capability::Update,
        &root.path().join("docs/wiki/0a1b2c3-glossary.md")
    ));
    assert_eq!(
        access.check(Capability::Read, &root.path().join("docs/rfd/042-x.md")),
        Err("access denied: read on docs/rfd/042-x.md".to_owned())
    );
    assert!(!access.allows(
        Capability::Create,
        &root.path().join("docs/wiki/0a1b2c4-new.md")
    ));
    assert!(!access.reads_workspace());
    assert!(access.is_restricted());
}

#[test]
fn the_workspace_root_is_what_a_root_rule_grants() {
    let root = Utf8TempDir::new().unwrap();
    let access = Access::granting(root.path(), vec![FsRule::new("").with_read(true)]);

    assert!(access.reads(root.path()));
    assert!(access.reads_workspace());
}

/// A tool reaches only the workspace, whatever its policy says about the rest.
#[test]
fn a_path_outside_the_workspace_is_refused() {
    let root = Utf8TempDir::new().unwrap();
    let elsewhere = Utf8TempDir::new().unwrap();
    let access = Access::granting(root.path(), vec![FsRule::new("").with_read(true)]);

    let refusal = access
        .check(Capability::Read, &elsewhere.path().join("x.md"))
        .unwrap_err();

    assert!(
        refusal.ends_with("/x.md is outside the workspace, which is all a tool reaches"),
        "{refusal}"
    );
}
