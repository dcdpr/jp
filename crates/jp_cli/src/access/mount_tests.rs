use super::*;

const CWD: &str = "/ws/sub";
const HOME: &str = "/home/u";

/// Resolver for a filesystem holding nothing.
fn nothing(_: &str) -> Option<Utf8PathBuf> {
    None
}

/// Resolver for a filesystem holding every path.
fn everything(path: &str) -> Option<Utf8PathBuf> {
    absolute_target(path, Utf8Path::new(CWD), Some(HOME))
}

/// Resolver for a filesystem holding only `existing`, matched as typed.
fn only<'a>(existing: &'a [&str]) -> impl Fn(&str) -> Option<Utf8PathBuf> + 'a {
    move |path| existing.contains(&path).then(|| everything(path)).flatten()
}

#[test]
fn parses_bare_name_defaults_readonly_all_tools() {
    let spec = MountSpec::parse("fork=~/code/forks/serde", nothing).unwrap();
    assert_eq!(spec.tool, None);
    assert_eq!(spec.name, "fork");
    assert_eq!(spec.path, "~/code/forks/serde");
    assert_eq!(spec.mode, MountMode::Ro);
}

#[test]
fn parses_explicit_readonly() {
    let spec = MountSpec::parse("fork=/p:ro", nothing).unwrap();
    assert_eq!(spec.mode, MountMode::Ro);
    assert_eq!(spec.path, "/p");
}

#[test]
fn parses_tool_scoped_readwrite() {
    let spec = MountSpec::parse("fs_modify_file:fork=/code/x:rw", nothing).unwrap();
    assert_eq!(spec.tool.as_deref(), Some("fs_modify_file"));
    assert_eq!(spec.name, "fork");
    assert_eq!(spec.path, "/code/x");
    assert_eq!(spec.mode, MountMode::Rw);
}

#[test]
fn readwrite_without_tool_is_rejected() {
    assert_eq!(
        MountSpec::parse("fork=/p:rw", nothing),
        Err(MountParseError::WriteRequiresTool("fork=/p:rw".to_owned()))
    );
}

#[test]
fn windows_drive_letter_on_right_is_not_a_tool() {
    // The `:` after `C` is on the right of `=`, so it is not a tool prefix and
    // the mode is peeled from the tail.
    let spec = MountSpec::parse("fs_read_file:fork=C:\\code\\forks\\x:ro", nothing).unwrap();
    assert_eq!(spec.tool.as_deref(), Some("fs_read_file"));
    assert_eq!(spec.path, "C:\\code\\forks\\x");
    assert_eq!(spec.mode, MountMode::Ro);
}

#[test]
fn explicit_form_never_consults_the_filesystem() {
    let spec = MountSpec::parse("fs_read_file:fork=/missing:ro", |path| {
        panic!("resolved {path:?} for an explicit NAME=PATH mount")
    })
    .unwrap();
    assert_eq!(spec.tool.as_deref(), Some("fs_read_file"));
    assert_eq!(spec.name, "fork");
    assert_eq!(spec.path, "/missing");
}

#[test]
fn bare_path_derives_name_from_basename() {
    let spec = MountSpec::parse(
        "/Users/jean/Projects/jp-todo",
        only(&["/Users/jean/Projects/jp-todo"]),
    )
    .unwrap();
    assert_eq!(spec.tool, None);
    assert_eq!(spec.name, "jp-todo");
    assert_eq!(spec.path, "/Users/jean/Projects/jp-todo");
    assert_eq!(spec.mode, MountMode::Ro);
}

#[test]
fn bare_home_relative_path_derives_name_from_basename() {
    let spec = MountSpec::parse("~/code/forks/serde", only(&["~/code/forks/serde"])).unwrap();
    assert_eq!(spec.name, "serde");
    assert_eq!(spec.path, "~/code/forks/serde");
}

#[test]
fn bare_path_with_trailing_slash_derives_name_from_last_segment() {
    let spec = MountSpec::parse("/code/forks/serde/", only(&["/code/forks/serde/"])).unwrap();
    assert_eq!(spec.name, "serde");
    assert_eq!(spec.path, "/code/forks/serde/");
}

#[test]
fn bare_path_name_is_normalized_lexically() {
    let spec = MountSpec::parse("/code/forks/../serde/.", everything).unwrap();
    assert_eq!(spec.name, "serde");
    assert_eq!(spec.path, "/code/forks/../serde/.");
}

#[test]
fn bare_dot_segments_and_tilde_are_named_after_the_directory_they_reach() {
    // The fake CWD is `/ws/sub` and the fake home is `/home/u`.
    let resolve = only(&[".", "..", "~"]);
    assert_eq!(MountSpec::parse(".", &resolve).unwrap().name, "sub");
    assert_eq!(MountSpec::parse("..", &resolve).unwrap().name, "ws");
    assert_eq!(MountSpec::parse("~", &resolve).unwrap().name, "u");
}

#[test]
fn bare_path_peels_mode_before_resolving() {
    let spec = MountSpec::parse("/code/forks/serde:ro", only(&["/code/forks/serde"])).unwrap();
    assert_eq!(spec.name, "serde");
    assert_eq!(spec.path, "/code/forks/serde");
    assert_eq!(spec.mode, MountMode::Ro);
}

#[test]
fn bare_path_readwrite_without_tool_is_rejected() {
    assert_eq!(
        MountSpec::parse("/code/forks/serde:rw", only(&["/code/forks/serde"])),
        Err(MountParseError::WriteRequiresTool(
            "/code/forks/serde:rw".to_owned()
        ))
    );
}

#[test]
fn bare_path_that_does_not_exist_is_rejected() {
    assert_eq!(
        MountSpec::parse("fork", nothing),
        Err(MountParseError::NotFound("fork".to_owned()))
    );
}

#[test]
fn bare_root_has_no_name() {
    for spec in ["/", "../.."] {
        assert_eq!(
            MountSpec::parse(spec, everything),
            Err(MountParseError::NoName(spec.to_owned())),
            "spec: {spec:?}"
        );
    }
}

#[test]
fn bare_empty_path_is_rejected_without_resolving_to_the_cwd() {
    // `everything` would resolve "" to the CWD; an empty PATH must not mount it.
    for spec in ["", ":ro"] {
        assert_eq!(
            MountSpec::parse(spec, everything),
            Err(MountParseError::Empty(spec.to_owned())),
            "spec: {spec:?}"
        );
    }
}

#[test]
fn bare_tool_prefix_scopes_the_mount_when_only_the_rest_exists() {
    let spec = MountSpec::parse("fs_modify_file:/code/x:rw", only(&["/code/x"])).unwrap();
    assert_eq!(spec.tool.as_deref(), Some("fs_modify_file"));
    assert_eq!(spec.name, "x");
    assert_eq!(spec.path, "/code/x");
    assert_eq!(spec.mode, MountMode::Rw);
}

#[test]
fn bare_tool_like_prefix_is_part_of_the_path_when_only_the_whole_exists() {
    let spec = MountSpec::parse("fs_read_file:x", only(&["fs_read_file:x"])).unwrap();
    assert_eq!(spec.tool, None);
    assert_eq!(spec.name, "fs_read_file:x");
    assert_eq!(spec.path, "fs_read_file:x");
}

#[test]
fn bare_tool_prefix_is_ambiguous_when_both_readings_exist() {
    assert_eq!(
        MountSpec::parse("fs_read_file:x", only(&["fs_read_file:x", "x"])),
        Err(MountParseError::Ambiguous {
            spec: "fs_read_file:x".to_owned(),
            tool: "fs_read_file".to_owned(),
        })
    );
}

#[test]
fn bare_lowercase_drive_letter_is_not_ambiguous() {
    // On Windows, `\code\x` resolves on the current drive, so both readings of
    // `c:\code\x` exist.
    let spec = MountSpec::parse("c:/code/x", only(&["c:/code/x", "/code/x"])).unwrap();
    assert_eq!(spec.tool, None);
    assert_eq!(spec.path, "c:/code/x");
    assert_eq!(spec.name, "x");
}

#[test]
fn bare_prefix_that_is_not_a_tool_name_is_never_a_scope() {
    // `C:` and `Bad-Tool:` are not tool identifiers, and a single letter is
    // read as a drive letter, so `/code/x` existing does not turn any of them
    // into a scope.
    for spec in ["C:/code/x", "c:/code/x", "Bad-Tool:/code/x"] {
        assert_eq!(
            MountSpec::parse(spec, only(&["/code/x"])),
            Err(MountParseError::NotFound(spec.to_owned())),
            "spec: {spec:?}"
        );
    }
}

#[test]
fn invalid_tool_identifier_is_rejected() {
    assert!(matches!(
        MountSpec::parse("Bad-Tool:fork=/p", nothing),
        Err(MountParseError::InvalidTool { .. })
    ));
}

#[test]
fn rule_reflects_mode() {
    let ro = MountSpec::parse("fork=/p", nothing).unwrap().rule("fork");
    assert_eq!(ro.write, Some(false));
    assert_eq!(ro.read, Some(true));
    assert_eq!(ro.external, Some(true));

    let rw = MountSpec::parse("t:fork=/p:rw", nothing)
        .unwrap()
        .rule("fork");
    assert_eq!(rw.write, Some(true));
}

#[test]
fn resolve_name_under_workspace() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse("foo/bar=/p", nothing).unwrap();
    let resolved = spec.resolve_name(Utf8Path::new("/ws"), root).unwrap();
    assert_eq!(resolved, Utf8PathBuf::from("foo/bar"));
}

#[test]
fn resolve_name_from_subdir_with_parent() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse("../qux/baz=/p", nothing).unwrap();
    let resolved = spec.resolve_name(Utf8Path::new("/ws/foo"), root).unwrap();
    assert_eq!(resolved, Utf8PathBuf::from("qux/baz"));
}

#[test]
fn resolve_name_escaping_workspace_is_rejected() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse("../baz=/p", nothing).unwrap();
    assert!(matches!(
        spec.resolve_name(Utf8Path::new("/ws"), root),
        Err(MountResolveError::OutsideWorkspace(_))
    ));
}

#[test]
fn resolve_name_absolute_under_workspace_is_trimmed() {
    let root = Utf8Path::new("/ws");
    let inside = MountSpec::parse("/ws/fork=/p", nothing).unwrap();
    assert_eq!(
        inside.resolve_name(Utf8Path::new("/ws"), root).unwrap(),
        Utf8PathBuf::from("fork")
    );

    let outside = MountSpec::parse("/elsewhere/fork=/p", nothing).unwrap();
    assert!(matches!(
        outside.resolve_name(Utf8Path::new("/ws"), root),
        Err(MountResolveError::OutsideWorkspace(_))
    ));
}

#[test]
fn resolve_name_into_managed_storage_is_rejected() {
    let root = Utf8Path::new("/ws");
    let spec = MountSpec::parse(".jp/x=/p", nothing).unwrap();
    assert!(matches!(
        spec.resolve_name(Utf8Path::new("/ws"), root),
        Err(MountResolveError::ManagedStorage(_))
    ));
}
