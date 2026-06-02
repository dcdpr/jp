use super::*;

fn fs_config(rules: Vec<FsRuleConfig>) -> AccessConfig {
    AccessConfig { fs: rules }
}

fn rule(path: &str) -> FsRuleConfig {
    FsRuleConfig {
        path: path.to_owned(),
        external: None,
        read: Some(true),
        write: None,
        create: None,
        update: None,
        delete: None,
        execute: None,
    }
}

#[test]
fn ordinary_rules_compile_without_approval() {
    let dir = camino_tempfile::tempdir().unwrap();
    let config = fs_config(vec![rule("."), rule("src")]);

    let compiled = compile_fs(&config, dir.path(), |_, _| {
        panic!("ordinary rules must not consult the approver")
    })
    .unwrap();

    assert_eq!(compiled.rules.len(), 2);
    assert!(compiled.warnings.is_empty());
    assert_eq!(compiled.rules[1].lexical_path(), "src");
    assert!(!compiled.rules[1].external());
}

#[test]
fn absolute_rule_path_is_rejected() {
    let dir = camino_tempfile::tempdir().unwrap();
    let config = fs_config(vec![rule("/etc")]);
    assert!(matches!(
        compile_fs(&config, dir.path(), |_, _| ApprovalDecision::Approved),
        Err(CompileError::NotWorkspaceRelative(_))
    ));
}

#[cfg(unix)]
#[test]
fn external_rule_approved_bakes_target() {
    use std::os::unix::fs::symlink;

    let workspace = camino_tempfile::tempdir().unwrap();
    let external = camino_tempfile::tempdir().unwrap();
    let external_canonical = external.path().canonicalize_utf8().unwrap();
    symlink(external.path(), workspace.path().join("fork")).unwrap();

    let mut external_rule = rule("fork");
    external_rule.external = Some(true);
    external_rule.write = Some(true);
    let config = fs_config(vec![external_rule]);

    let compiled = compile_fs(&config, workspace.path(), |path, candidate| {
        assert_eq!(path, "fork");
        assert_eq!(candidate, external_canonical);
        ApprovalDecision::Approved
    })
    .unwrap();

    assert_eq!(compiled.rules.len(), 1);
    let compiled_rule = &compiled.rules[0];
    assert!(compiled_rule.external());
    assert_eq!(
        compiled_rule.approved_target(),
        Some(external_canonical.as_path())
    );
    assert!(compiled_rule.update());
}

#[cfg(unix)]
#[test]
fn external_rule_rejected_is_dropped_with_warning() {
    use std::os::unix::fs::symlink;

    let workspace = camino_tempfile::tempdir().unwrap();
    let external = camino_tempfile::tempdir().unwrap();
    symlink(external.path(), workspace.path().join("fork")).unwrap();

    let mut external_rule = rule("fork");
    external_rule.external = Some(true);
    let config = fs_config(vec![external_rule]);

    let compiled =
        compile_fs(&config, workspace.path(), |_, _| ApprovalDecision::Rejected).unwrap();

    assert!(compiled.rules.is_empty());
    assert_eq!(compiled.warnings.len(), 1);
}

#[cfg(unix)]
#[test]
fn external_rule_resolving_inside_workspace_is_rejected() {
    use std::os::unix::fs::symlink;

    let workspace = camino_tempfile::tempdir().unwrap();
    let canonical = workspace.path().canonicalize_utf8().unwrap();
    std::fs::create_dir(canonical.join("real")).unwrap();
    // `link` points back inside the workspace.
    symlink(canonical.join("real"), canonical.join("link")).unwrap();

    let mut external_rule = rule("link");
    external_rule.external = Some(true);
    let config = fs_config(vec![external_rule]);

    assert!(matches!(
        compile_fs(&config, workspace.path(), |_, _| ApprovalDecision::Approved),
        Err(CompileError::ExternalInsideWorkspace(_))
    ));
}

#[test]
fn external_rule_with_broken_symlink_is_dropped() {
    // No symlink on disk: canonicalization fails → drop with a warning.
    let workspace = camino_tempfile::tempdir().unwrap();
    let mut external_rule = rule("missing");
    external_rule.external = Some(true);
    let config = fs_config(vec![external_rule]);

    let compiled =
        compile_fs(&config, workspace.path(), |_, _| ApprovalDecision::Approved).unwrap();
    assert!(compiled.rules.is_empty());
    assert_eq!(compiled.warnings.len(), 1);
}
