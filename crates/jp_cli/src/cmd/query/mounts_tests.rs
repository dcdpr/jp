use jp_config::conversation::tool::access::PartialEnvRuleConfig;

use super::*;

/// An `access` block granting read on one workspace path.
fn fs_access(path: &str) -> PartialAccessConfig {
    PartialAccessConfig {
        fs: vec![PartialFsRuleConfig {
            path: Some(path.to_owned()),
            read: Some(true),
            ..Default::default()
        }]
        .into(),
        ..PartialAccessConfig::default()
    }
}

/// An `access` block granting read on one environment variable.
fn env_access(name: &str) -> PartialAccessConfig {
    PartialAccessConfig {
        env: vec![PartialEnvRuleConfig {
            name: Some(name.to_owned()),
            read: Some(true),
        }]
        .into(),
        ..PartialAccessConfig::default()
    }
}

/// A partial tools config with one local tool `my_tool`, carrying the given
/// `'*'` and per-tool `access` blocks.
fn tools_with_access(
    defaults: Option<PartialAccessConfig>,
    tool: Option<PartialAccessConfig>,
) -> PartialToolsConfig {
    let mut tools = PartialToolsConfig::default();
    tools.defaults.access = defaults;
    tools.tools.insert("my_tool".to_owned(), PartialToolConfig {
        source: Some(ToolSource::Local { tool: None }),
        access: tool,
        ..Default::default()
    });
    tools
}

/// With nothing declared anywhere, the tool had unrestricted workspace access,
/// so the seed stands in for it rather than letting the mount rule become the
/// tool's whole policy.
#[test]
fn mount_seeds_the_workspace_rule_when_no_access_is_declared() {
    let tools = tools_with_access(None, None);

    let seed = mount_access_seed(&tools, "my_tool");

    assert_eq!(seed.fs.len(), 1);
    assert_eq!(seed.fs[0].path.as_deref(), Some("."));
    assert_eq!(seed.fs[0].read, Some(true));
    assert_eq!(seed.fs[0].write, Some(true));
    assert!(seed.env.is_empty());
}

/// The tool was inheriting the `'*'` rules, and writing into its own scope
/// detaches it from them, so they are copied across.
/// Seeding the workspace-wide rule here would hand the tool back the access
/// `'*'` took away.
#[test]
fn mount_seeds_the_defaults_rules_when_the_tool_declares_none() {
    let tools = tools_with_access(Some(fs_access("src")), None);

    let seed = mount_access_seed(&tools, "my_tool");

    assert_eq!(seed.fs.len(), 1);
    assert_eq!(seed.fs[0].path.as_deref(), Some("src"));
}

/// The whole `'*'` block is carried, not just its filesystem rules: the tool's
/// `env` restrictions came from `'*'` and would otherwise be dropped, leaving
/// it able to read every variable.
///
/// The `'*'` block restricts no path, so the tool also keeps the workspace-wide
/// `fs` rule — without it the mount rule alone would switch the tool's
/// filesystem access to default-deny.
#[test]
fn mount_seeds_the_defaults_env_rules_and_keeps_filesystem_reach() {
    let tools = tools_with_access(Some(env_access("AWS_*")), None);

    let seed = mount_access_seed(&tools, "my_tool");

    assert_eq!(seed.env.len(), 1);
    assert_eq!(seed.env[0].name.as_deref(), Some("AWS_*"));
    assert_eq!(seed.fs.len(), 1);
    assert_eq!(seed.fs[0].path.as_deref(), Some("."));
}

/// A tool with filesystem rules of its own is already detached from `'*'`; its
/// policy is complete as written and the mount rule just appends to it.
#[test]
fn mount_seeds_nothing_when_the_tool_declares_its_own_access() {
    let tools = tools_with_access(Some(fs_access("src")), Some(fs_access("vendor")));

    let seed = mount_access_seed(&tools, "my_tool");

    assert!(
        seed.fs.is_empty(),
        "the tool's own rules are already stored"
    );
    assert!(seed.env.is_empty());
}

/// A tool declaring only `env` rules restricts no path, so it still reaches the
/// whole workspace.
/// Its own rules are not copied, but the workspace-wide `fs` rule is, so the
/// mount doesn't take that reach away.
#[test]
fn mount_keeps_filesystem_reach_for_a_tool_declaring_only_env_rules() {
    let tools = tools_with_access(None, Some(env_access("GITHUB_TOKEN")));

    let seed = mount_access_seed(&tools, "my_tool");

    assert_eq!(seed.fs.len(), 1);
    assert_eq!(seed.fs[0].path.as_deref(), Some("."));
    assert!(
        seed.env.is_empty(),
        "the tool's own env rules are already stored"
    );
}

/// A tool absent from the config declares nothing, so it is seeded like any
/// other undeclared tool.
#[test]
fn mount_seeds_an_unconfigured_tool_from_the_defaults() {
    let tools = tools_with_access(Some(fs_access("src")), None);

    let seed = mount_access_seed(&tools, "absent_tool");

    assert_eq!(seed.fs.len(), 1);
    assert_eq!(seed.fs[0].path.as_deref(), Some("src"));
}

/// Which tools a bare `--mount` (no `TOOL:` prefix) grants the mounted path to.
///
/// Every tool that runs as a subprocess JP starts gets it, and nothing else: a
/// local or command plugin tool left out would lose access the user mounted for
/// it, and a builtin or MCP tool given a rule would carry a grant nothing
/// enforces.
#[test]
fn a_bare_mount_reaches_every_enabled_subprocess_tool() {
    let mut partial = PartialAppConfig::new_test();
    for (name, source) in [
        ("local_tool", ToolSource::Local { tool: None }),
        ("plugin_tool", ToolSource::CommandPlugin {
            plugin: "ticket".to_owned(),
            tool: None,
        }),
        ("mcp_tool", ToolSource::Mcp {
            server: "server".to_owned(),
            tool: None,
        }),
        ("builtin_tool", ToolSource::Builtin { tool: None }),
    ] {
        partial
            .conversation
            .tools
            .tools
            .insert(name.to_owned(), PartialToolConfig {
                source: Some(source),
                ..Default::default()
            });
    }

    inject_mounts(
        &mut partial,
        &["notes=/elsewhere/notes".to_owned()],
        Utf8Path::new("/ws"),
        Utf8Path::new("/ws"),
        |_| None,
        None,
    )
    .unwrap();

    let mut granted: Vec<&str> = partial
        .conversation
        .tools
        .tools
        .iter()
        .filter(|(_, cfg)| {
            cfg.access.as_ref().is_some_and(|access| {
                access
                    .fs
                    .iter()
                    .any(|rule| rule.path.as_deref() == Some("notes"))
            })
        })
        .map(|(name, _)| name.as_str())
        .collect();
    granted.sort_unstable();

    assert_eq!(granted, ["local_tool", "plugin_tool"]);
}

/// A name derived from a bare `--mount PATH` often collides with something
/// already in the workspace, so the error says how to pick another name.
#[test]
fn mount_onto_an_existing_entry_suggests_naming_it() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let link = tmp.path().join("src");
    let target = tmp.path().join("target");
    std::fs::create_dir(&link).unwrap();
    std::fs::create_dir(&target).unwrap();

    let error = create_workspace_symlink(&link, &target).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "CLI Config error: cannot create mount: '{link}' already exists and is not a symlink; \
             choose another name with NAME=PATH"
        )
    );
}

#[cfg(unix)]
#[test]
fn mount_onto_a_symlink_pointing_elsewhere_suggests_naming_it() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let link = tmp.path().join("src");
    let elsewhere = tmp.path().join("elsewhere");
    let target = tmp.path().join("target");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();

    let error = create_workspace_symlink(&link, &target).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "CLI Config error: mount '{link}' already exists as a symlink pointing elsewhere; \
             choose another name with NAME=PATH"
        )
    );
}
