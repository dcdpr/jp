use super::*;

#[test]
fn parses_bare_name_defaults_readonly_all_tools() {
    let spec = MountSpec::parse("fork=~/code/forks/serde").unwrap();
    assert_eq!(spec.tool, None);
    assert_eq!(spec.name, "fork");
    assert_eq!(spec.path, "~/code/forks/serde");
    assert_eq!(spec.mode, MountMode::Ro);
}

#[test]
fn parses_explicit_readonly() {
    let spec = MountSpec::parse("fork=/p:ro").unwrap();
    assert_eq!(spec.mode, MountMode::Ro);
    assert_eq!(spec.path, "/p");
}

#[test]
fn parses_tool_scoped_readwrite() {
    let spec = MountSpec::parse("fs_modify_file:fork=/code/x:rw").unwrap();
    assert_eq!(spec.tool.as_deref(), Some("fs_modify_file"));
    assert_eq!(spec.name, "fork");
    assert_eq!(spec.path, "/code/x");
    assert_eq!(spec.mode, MountMode::Rw);
}

#[test]
fn readwrite_without_tool_is_rejected() {
    assert_eq!(
        MountSpec::parse("fork=/p:rw"),
        Err(MountParseError::WriteRequiresTool("fork=/p:rw".to_owned()))
    );
}

#[test]
fn windows_drive_letter_on_right_is_not_a_tool() {
    // The `:` after `C` is on the right of `=`, so it is not a tool prefix and
    // the mode is peeled from the tail.
    let spec = MountSpec::parse("fs_read_file:fork=C:\\code\\forks\\x:ro").unwrap();
    assert_eq!(spec.tool.as_deref(), Some("fs_read_file"));
    assert_eq!(spec.path, "C:\\code\\forks\\x");
    assert_eq!(spec.mode, MountMode::Ro);
}

#[test]
fn missing_equals_is_rejected() {
    assert!(matches!(
        MountSpec::parse("fork"),
        Err(MountParseError::MissingEquals(_))
    ));
}

#[test]
fn invalid_tool_identifier_is_rejected() {
    assert!(matches!(
        MountSpec::parse("Bad-Tool:fork=/p"),
        Err(MountParseError::InvalidTool { .. })
    ));
}

#[test]
fn rule_reflects_mode() {
    let ro = MountSpec::parse("fork=/p").unwrap().rule("fork");
    assert_eq!(ro.write, Some(false));
    assert_eq!(ro.read, Some(true));
    assert_eq!(ro.external, Some(true));

    let rw = MountSpec::parse("t:fork=/p:rw").unwrap().rule("fork");
    assert_eq!(rw.write, Some(true));
}

#[test]
fn resolve_name_under_workspace() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse("foo/bar=/p").unwrap();
    let resolved = spec.resolve_name(Utf8Path::new("/ws"), root).unwrap();
    assert_eq!(resolved, Utf8PathBuf::from("foo/bar"));
}

#[test]
fn resolve_name_from_subdir_with_parent() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse("../qux/baz=/p").unwrap();
    let resolved = spec.resolve_name(Utf8Path::new("/ws/foo"), root).unwrap();
    assert_eq!(resolved, Utf8PathBuf::from("qux/baz"));
}

#[test]
fn resolve_name_escaping_workspace_is_rejected() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse("../baz=/p").unwrap();
    assert!(matches!(
        spec.resolve_name(Utf8Path::new("/ws"), root),
        Err(MountResolveError::OutsideWorkspace(_))
    ));
}

#[test]
fn resolve_name_absolute_is_rejected() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse("/abs/fork=/p").unwrap();
    assert!(matches!(
        spec.resolve_name(Utf8Path::new("/ws"), root),
        Err(MountResolveError::NotRelative(_))
    ));
}

#[test]
fn resolve_name_into_managed_storage_is_rejected() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse(".jp/x=/p").unwrap();
    assert!(matches!(
        spec.resolve_name(Utf8Path::new("/ws"), root),
        Err(MountResolveError::ManagedStorage(_))
    ));
}
