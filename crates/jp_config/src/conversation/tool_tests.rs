use assert_matches::assert_matches;
use schematic::{SchemaBuilder, SchemaType};
use serde_json::json;

use super::*;
use crate::types::json_value::JsonValue;

#[test]
fn access_on_mcp_tool_is_rejected_by_validation() {
    use crate::{
        PartialAppConfig,
        conversation::tool::access::{PartialAccessConfig, PartialFsRuleConfig},
        util::build,
    };

    let mut partial = PartialAppConfig::new_test();
    partial
        .conversation
        .tools
        .tools
        .insert("my_mcp".to_owned(), PartialToolConfig {
            source: Some(ToolSource::Mcp {
                server: "server".to_owned(),
                tool: None,
            }),
            access: Some(PartialAccessConfig {
                fs: vec![PartialFsRuleConfig {
                    path: Some(".".to_owned()),
                    read: Some(true),
                    ..Default::default()
                }]
                .into(),
            }),
            ..Default::default()
        });

    let err = build(partial).unwrap_err().to_string();
    assert!(
        err.contains("only supported on local tools"),
        "unexpected error: {err}"
    );
}

#[test]
fn access_on_local_tool_is_accepted_by_validation() {
    use crate::{
        PartialAppConfig,
        conversation::tool::access::{PartialAccessConfig, PartialFsRuleConfig},
        util::build,
    };

    let mut partial = PartialAppConfig::new_test();
    partial
        .conversation
        .tools
        .tools
        .insert("my_local".to_owned(), PartialToolConfig {
            source: Some(ToolSource::Local { tool: None }),
            access: Some(PartialAccessConfig {
                fs: vec![PartialFsRuleConfig {
                    path: Some(".".to_owned()),
                    read: Some(true),
                    ..Default::default()
                }]
                .into(),
            }),
            ..Default::default()
        });

    assert!(build(partial).is_ok());
}

#[test]
fn test_enable_config_from_bool() {
    assert_eq!(PartialEnableConfig::from(true), PartialEnableConfig::ON);
    assert_eq!(PartialEnableConfig::from(false), PartialEnableConfig::OFF);
}

#[test]
fn test_enable_config_from_str() {
    assert_eq!(
        "true".parse::<PartialEnableConfig>(),
        Ok(PartialEnableConfig::ON)
    );
    assert_eq!(
        "on".parse::<PartialEnableConfig>(),
        Ok(PartialEnableConfig::ON)
    );
    assert_eq!(
        "false".parse::<PartialEnableConfig>(),
        Ok(PartialEnableConfig::OFF)
    );
    assert_eq!(
        "off".parse::<PartialEnableConfig>(),
        Ok(PartialEnableConfig::OFF)
    );
    // Legacy `always` is locked-on; legacy `explicit` is off-unless-named.
    assert_eq!(
        "always".parse::<PartialEnableConfig>(),
        Ok(PartialEnableConfig::LOCKED_ON)
    );
    assert_eq!(
        "explicit".parse::<PartialEnableConfig>(),
        Ok(PartialEnableConfig {
            state: Some(false),
            allow_toggle: Some(AllowToggle::IfNamed),
        })
    );
    assert!("invalid".parse::<PartialEnableConfig>().is_err());
}

#[test]
fn test_enable_config_serialize() {
    // Output is always the table form (auto-derived), carrying only the fields
    // that are set. The bool / legacy-string forms are input-only.
    assert_eq!(
        serde_json::to_value(PartialEnableConfig::ON).unwrap(),
        json!({ "state": true, "allow_toggle": "any" })
    );
    assert_eq!(
        serde_json::to_value(PartialEnableConfig::OFF).unwrap(),
        json!({ "state": false, "allow_toggle": "any" })
    );
    assert_eq!(
        serde_json::to_value(PartialEnableConfig::LOCKED_ON).unwrap(),
        json!({ "state": true, "allow_toggle": "never" })
    );
    assert_eq!(
        serde_json::to_value(PartialEnableConfig::LOCKED_OFF).unwrap(),
        json!({ "state": false, "allow_toggle": "never" })
    );

    // Unset fields are omitted, preserving inheritance from lower layers.
    assert_eq!(
        serde_json::to_value(PartialEnableConfig {
            state: Some(true),
            allow_toggle: None,
        })
        .unwrap(),
        json!({ "state": true })
    );
    assert_eq!(
        serde_json::to_value(PartialEnableConfig {
            state: None,
            allow_toggle: Some(AllowToggle::IfNamed),
        })
        .unwrap(),
        json!({ "allow_toggle": "if_named" })
    );
    assert_eq!(
        serde_json::to_value(PartialEnableConfig::default()).unwrap(),
        json!({})
    );
}

#[test]
fn test_enable_config_deserialize() {
    // Bool fills both fields.
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(json!(true)).unwrap(),
        PartialEnableConfig::ON
    );
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(json!(false)).unwrap(),
        PartialEnableConfig::OFF
    );

    // Legacy strings fill both fields.
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(json!("on")).unwrap(),
        PartialEnableConfig::ON
    );
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(json!("always")).unwrap(),
        PartialEnableConfig::LOCKED_ON
    );
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(json!("explicit")).unwrap(),
        PartialEnableConfig {
            state: Some(false),
            allow_toggle: Some(AllowToggle::IfNamed),
        }
    );

    // The table form preserves omitted fields as `None`.
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(json!({ "state": true })).unwrap(),
        PartialEnableConfig {
            state: Some(true),
            allow_toggle: None,
        }
    );
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(
            json!({ "allow_toggle": "if_named_or_group" })
        )
        .unwrap(),
        PartialEnableConfig {
            state: None,
            allow_toggle: Some(AllowToggle::IfNamedOrGroup),
        }
    );
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(
            json!({ "state": false, "allow_toggle": "never" })
        )
        .unwrap(),
        PartialEnableConfig::LOCKED_OFF
    );

    // `any` is the freely-toggleable spelling; legacy `always` is NOT accepted
    // for `allow_toggle` (as a top-level value it means locked-on instead).
    assert_eq!(
        serde_json::from_value::<PartialEnableConfig>(json!({ "allow_toggle": "any" })).unwrap(),
        PartialEnableConfig {
            state: None,
            allow_toggle: Some(AllowToggle::Always),
        }
    );
    assert!(
        serde_json::from_value::<PartialEnableConfig>(json!({ "allow_toggle": "always" })).is_err()
    );
}

#[test]
fn test_format_mode_survives_merge_with_persona_override() {
    use schematic::PartialConfig as _;

    // Simulate the real-world layering: MCP tool TOML sets `format =
    // "unattended"`; persona later sets only `enable = true`. The
    // resulting merged config must still have `format = Unattended`.
    let mcp_toml = r#"
source = "local"
enable = false
format = "unattended"
command = "just"
"#;
    let persona_toml = r"
enable = true
";

    let mut base: PartialToolConfig = toml::from_str(mcp_toml).unwrap();
    let next: PartialToolConfig = toml::from_str(persona_toml).unwrap();

    base.merge(&(), next).unwrap();

    assert_eq!(base.format, Some(FormatMode::Unattended));
    assert_eq!(base.enable, Some(PartialEnableConfig::ON));
}

#[test]
fn test_format_mode_deserializes_from_toml() {
    let toml = r#"
source = "local"
enable = false
format = "unattended"
run = "ask"
"#;
    let partial: PartialToolConfig = toml::from_str(toml).unwrap();
    assert_eq!(partial.format, Some(FormatMode::Unattended));
    assert_eq!(partial.run, Some(RunMode::Ask));
}

#[test]
fn test_enable_assign_kv() {
    let mut p = PartialToolConfig::default_values(&()).unwrap().unwrap();

    // Assign via string "true" (bool shorthand fills both fields).
    let kv = KvAssignment::try_from_cli("enable", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.enable, Some(PartialEnableConfig::ON));

    // Assign via legacy string "explicit".
    let kv = KvAssignment::try_from_cli("enable", "explicit").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.enable,
        Some(PartialEnableConfig {
            state: Some(false),
            allow_toggle: Some(AllowToggle::IfNamed),
        })
    );

    // Assign via JSON bool.
    let kv = KvAssignment::try_from_cli("enable:", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.enable, Some(PartialEnableConfig::OFF));

    // Nested assignment of a single subfield preserves the other field.
    let kv = KvAssignment::try_from_cli("enable.allow_toggle", "if_named").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.enable,
        Some(PartialEnableConfig {
            state: Some(false),
            allow_toggle: Some(AllowToggle::IfNamed),
        })
    );

    let kv = KvAssignment::try_from_cli("enable.state", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.enable,
        Some(PartialEnableConfig {
            state: Some(true),
            allow_toggle: Some(AllowToggle::IfNamed),
        })
    );
}

#[test]
fn test_tools_config() {
    assert_matches!(PartialToolsConfig::default_values(&()), Ok(Some(_)));
    assert_matches!(PartialToolConfig::default_values(&()), Ok(Some(_)));

    let mut p = PartialToolsConfig::default_values(&()).unwrap().unwrap();

    p.tools.insert("cargo_check".to_owned(), PartialToolConfig {
        enable: Some(PartialEnableConfig::OFF),
        source: Some(ToolSource::Local { tool: None }),
        ..Default::default()
    });

    let kv = KvAssignment::try_from_cli("cargo_check.enable", "true").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(
        p.tools,
        IndexMap::<_, _>::from_iter(vec![("cargo_check".to_owned(), PartialToolConfig {
            enable: Some(PartialEnableConfig::ON),
            source: Some(ToolSource::Local { tool: None }),
            ..Default::default()
        })])
    );

    let kv = KvAssignment::try_from_cli("foo:", r#"{"source":"builtin"}"#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.tools,
        IndexMap::<_, _>::from_iter(vec![
            ("cargo_check".to_owned(), PartialToolConfig {
                enable: Some(PartialEnableConfig::ON),
                source: Some(ToolSource::Local { tool: None }),
                ..Default::default()
            }),
            ("foo".to_owned(), PartialToolConfig {
                source: Some(ToolSource::Builtin { tool: None }),
                ..Default::default()
            })
        ])
    );
}

#[test]
fn test_tool_config_command() {
    let mut p = PartialToolConfig::default_values(&()).unwrap().unwrap();
    assert!(p.command.is_none());

    let kv = KvAssignment::try_from_cli("command", "cargo check").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.command,
        Some(PartialCommandConfigOrString::String(
            "cargo check".to_owned()
        ))
    );

    let cfg = CommandConfigOrString::from_partial(p.command.clone().unwrap(), vec![]).unwrap();
    assert_eq!(cfg.command(), CommandConfig {
        program: "cargo".to_owned(),
        args: vec!["check".to_owned()],
        shell: false,
    });

    let kv = KvAssignment::try_from_cli(
        "command:",
        r#"{"program":"cargo","args":["check", "--verbose"],"shell":true}"#,
    )
    .unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.command,
        Some(PartialCommandConfigOrString::Config(PartialCommandConfig {
            program: Some("cargo".to_owned()),
            args: Some(vec!["check".to_owned(), "--verbose".to_owned()]),
            shell: Some(true),
        }))
    );

    let cfg = CommandConfigOrString::from_partial(p.command.unwrap(), vec![]).unwrap();
    assert_eq!(cfg.command(), CommandConfig {
        program: "cargo".to_owned(),
        args: vec!["check".to_owned(), "--verbose".to_owned()],
        shell: true,
    });
}

#[test]
fn test_enable_schema() {
    // `EnableConfig`'s root schema is a struct exposing `state` and
    // `allow_toggle` as real fields (not a bool|string union like the old
    // `Enable`). This is the type's own schema: in `AppConfig::fields()` the
    // `enable` field stays a flat leaf (`conversation.tools.*.enable`) because
    // it's a `no_deserialize_derive` config.
    let schema = SchemaBuilder::build_root::<EnableConfig>();
    let SchemaType::Struct(s) = schema.ty else {
        panic!("expected struct, got {:?}", schema.ty)
    };
    assert!(s.fields.contains_key("state"), "missing `state` field");
    assert!(
        s.fields.contains_key("allow_toggle"),
        "missing `allow_toggle` field"
    );
}

#[test]
fn test_tool_config_json_merge_preserves_existing_fields() {
    let mut p = PartialToolsConfig::default_values(&()).unwrap().unwrap();

    // Pre-populate a tool with source and command.
    p.tools.insert("my_tool".to_owned(), PartialToolConfig {
        source: Some(ToolSource::Local { tool: None }),
        command: Some(PartialCommandConfigOrString::String(
            "my-command".to_owned(),
        )),
        enable: Some(PartialEnableConfig::OFF),
        ..Default::default()
    });

    // Override only enable and run via a JSON object.
    let kv =
        KvAssignment::try_from_cli("my_tool:", r#"{"enable":true,"run":"unattended"}"#).unwrap();
    p.assign(kv).unwrap();

    let tool = p.tools.get("my_tool").unwrap();
    assert_eq!(tool.enable, Some(PartialEnableConfig::ON));
    assert_eq!(tool.run, Some(RunMode::Unattended));
    // These must survive the merge.
    assert_eq!(tool.source, Some(ToolSource::Local { tool: None }));
    assert_eq!(
        tool.command,
        Some(PartialCommandConfigOrString::String(
            "my-command".to_owned()
        ))
    );
}

#[test]
fn test_tool_config_json_merge_null_clears_field() {
    let mut p = PartialToolsConfig::default_values(&()).unwrap().unwrap();

    p.tools.insert("t".to_owned(), PartialToolConfig {
        source: Some(ToolSource::Local { tool: None }),
        enable: Some(PartialEnableConfig::ON),
        run: Some(RunMode::Edit),
        summary: Some("keep me".to_owned()),
        ..Default::default()
    });

    // null clears enable and run; source and summary survive.
    let kv = KvAssignment::try_from_cli("t:", r#"{"enable":null,"run":null}"#).unwrap();
    p.assign(kv).unwrap();

    let tool = p.tools.get("t").unwrap();
    assert_eq!(tool.enable, None);
    assert_eq!(tool.run, None);
    assert_eq!(tool.source, Some(ToolSource::Local { tool: None }));
    assert_eq!(tool.summary, Some("keep me".to_owned()));
}

#[test]
fn test_tool_config_json_merge_nested_object() {
    let mut p = PartialToolsConfig::default_values(&()).unwrap().unwrap();

    p.tools.insert("t".to_owned(), PartialToolConfig {
        source: Some(ToolSource::Local { tool: None }),
        enable: Some(PartialEnableConfig::OFF),
        ..Default::default()
    });

    // Nested object for style is forwarded to DisplayStyleConfig::assign.
    let kv =
        KvAssignment::try_from_cli("t:", r#"{"enable":true,"style":{"hidden":true}}"#).unwrap();
    p.assign(kv).unwrap();

    let tool = p.tools.get("t").unwrap();
    assert_eq!(tool.enable, Some(PartialEnableConfig::ON));
    assert_eq!(tool.source, Some(ToolSource::Local { tool: None }));

    let style = tool.style.as_ref().unwrap();
    assert_eq!(style.hidden, Some(true));
}

#[test]
fn test_tool_config_json_merge_unknown_key_errors() {
    let mut p = PartialToolsConfig::default_values(&()).unwrap().unwrap();
    p.tools.insert("t".to_owned(), PartialToolConfig::default());

    let kv = KvAssignment::try_from_cli("t:", r#"{"bogus":"val"}"#).unwrap();
    let err = p.assign(kv).unwrap_err().to_string();
    assert!(err.contains("bogus"), "expected 'bogus' in error: {err}");
}

#[test]
fn test_tool_source_schema() {
    let schema = SchemaBuilder::build_root::<ToolSource>();
    assert_eq!(schema.name, Some("tool_source".to_owned()));
    assert_eq!(schema.ty, SchemaType::String(Box::default()));
}

#[test]
fn test_tool_source_mcp_parses_server_only() {
    let parsed: ToolSource = "mcp.bookworm".parse().unwrap();
    assert_eq!(parsed, ToolSource::Mcp {
        server: "bookworm".to_owned(),
        tool: None,
    });
}

#[test]
fn test_tool_source_mcp_parses_server_and_tool() {
    let parsed: ToolSource = "mcp.bookworm.crate_readme".parse().unwrap();
    assert_eq!(parsed, ToolSource::Mcp {
        server: "bookworm".to_owned(),
        tool: Some("crate_readme".to_owned()),
    });
}

#[test]
fn test_tool_source_mcp_rejects_legacy_no_server() {
    let err = "mcp".parse::<ToolSource>().unwrap_err();
    assert!(err.contains("must name a server"), "got: {err}");
}

#[test]
fn test_tool_source_mcp_rejects_legacy_empty_server() {
    let err = "mcp..read_file".parse::<ToolSource>().unwrap_err();
    assert!(err.contains("must name a server"), "got: {err}");
}

#[test]
fn test_tool_source_mcp_roundtrip_with_tool() {
    let original = ToolSource::Mcp {
        server: "bookworm".to_owned(),
        tool: Some("crate_readme".to_owned()),
    };
    let serialized = serde_json::to_string(&original).unwrap();
    assert_eq!(serialized, r#""mcp.bookworm.crate_readme""#);

    let parsed: ToolSource = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn test_tool_source_mcp_roundtrip_without_tool() {
    let original = ToolSource::Mcp {
        server: "bookworm".to_owned(),
        tool: None,
    };
    let serialized = serde_json::to_string(&original).unwrap();
    assert_eq!(serialized, r#""mcp.bookworm""#);

    let parsed: ToolSource = serde_json::from_str(&serialized).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn test_tool_options_assign_flat() {
    let mut p = PartialToolConfig::default();
    let kv = KvAssignment::try_from_cli("options.verbose", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.options["verbose"], JsonValue(json!("true")));
}

#[test]
fn test_tool_options_assign_nested() {
    let mut p = PartialToolConfig::default();
    let kv = KvAssignment::try_from_cli("options.web.port", "3000").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.options["web"], JsonValue(json!({"port": "3000"})));
}

#[test]
fn test_tool_options_assign_preserves_siblings() {
    let mut p = PartialToolConfig::default();
    p.options.insert("debug".to_owned(), JsonValue(json!(true)));

    let kv = KvAssignment::try_from_cli("options.verbose", "true").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(p.options["debug"], JsonValue(json!(true)));
    assert_eq!(p.options["verbose"], JsonValue(json!("true")));
}

#[test]
fn test_enable_effective_fills_from_defaults_then_fallback() {
    // Nothing set anywhere: enabled, freely toggleable.
    assert_eq!(
        PartialEnableConfig::default().effective(&PartialEnableConfig::default()),
        Enable {
            state: true,
            allow_toggle: AllowToggle::Always,
        }
    );

    // Per-tool overrides only `state`; `allow_toggle` comes from defaults.
    let tool = PartialEnableConfig {
        state: Some(true),
        allow_toggle: None,
    };
    let defaults = PartialEnableConfig {
        state: Some(false),
        allow_toggle: Some(AllowToggle::IfNamed),
    };
    assert_eq!(tool.effective(&defaults), Enable {
        state: true,
        allow_toggle: AllowToggle::IfNamed,
    });
}

#[test]
fn test_enable_accepts_by_scope() {
    let locked = Enable {
        state: true,
        allow_toggle: AllowToggle::Never,
    };
    assert!(locked.is_locked());
    assert!(!locked.accepts(ToggleScope::Bulk));
    assert!(!locked.accepts(ToggleScope::Named));

    let any = Enable {
        state: false,
        allow_toggle: AllowToggle::Always,
    };
    assert!(any.accepts(ToggleScope::Bulk));
    assert!(any.accepts(ToggleScope::Named));

    let if_named = Enable {
        state: false,
        allow_toggle: AllowToggle::IfNamed,
    };
    assert!(!if_named.accepts(ToggleScope::Bulk));
    assert!(if_named.accepts(ToggleScope::Named));
    assert!(!if_named.accepts(ToggleScope::NamedGroup));

    let group = Enable {
        state: false,
        allow_toggle: AllowToggle::IfNamedOrGroup,
    };
    assert!(!group.accepts(ToggleScope::Bulk));
    assert!(group.accepts(ToggleScope::Named));
    assert!(group.accepts(ToggleScope::NamedGroup));
}

#[test]
fn test_enable_cross_layer_merge_preserves_other_field() {
    use schematic::PartialConfig as _;

    // Lower layer sets only `state`; higher layer sets only `allow_toggle`.
    // Per-field merge keeps both.
    let mut base: PartialToolConfig = toml::from_str("enable = { state = true }").unwrap();
    let next: PartialToolConfig = toml::from_str(r#"enable = { allow_toggle = "never" }"#).unwrap();
    base.merge(&(), next).unwrap();
    assert_eq!(
        base.enable,
        Some(PartialEnableConfig {
            state: Some(true),
            allow_toggle: Some(AllowToggle::Never),
        })
    );

    // A bool in a higher layer fully specifies both fields, overriding any
    // inherited `allow_toggle`.
    let mut base: PartialToolConfig =
        toml::from_str(r#"enable = { allow_toggle = "if_named" }"#).unwrap();
    let next: PartialToolConfig = toml::from_str("enable = true").unwrap();
    base.merge(&(), next).unwrap();
    assert_eq!(base.enable, Some(PartialEnableConfig::ON));
}

#[test]
fn test_enable_cross_key_defaults_inheritance() {
    use crate::{PartialAppConfig, util::build};

    let mut partial = PartialAppConfig::new_test();
    partial.conversation.tools.defaults.enable = Some(PartialEnableConfig {
        state: Some(false),
        allow_toggle: Some(AllowToggle::IfNamed),
    });
    partial
        .conversation
        .tools
        .tools
        .insert("foo".to_owned(), PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            enable: Some(PartialEnableConfig {
                state: Some(true),
                allow_toggle: None,
            }),
            ..Default::default()
        });
    partial
        .conversation
        .tools
        .tools
        .insert("bar".to_owned(), PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            ..Default::default()
        });

    let config = build(partial).unwrap();
    let tools = &config.conversation.tools;

    // `foo` overrides only `state`; `allow_toggle` inherits from `*`.
    assert_eq!(tools.get("foo").unwrap().effective_enable(), Enable {
        state: true,
        allow_toggle: AllowToggle::IfNamed,
    });

    // `bar` sets no `enable`; both fields inherit from `*`.
    assert_eq!(tools.get("bar").unwrap().effective_enable(), Enable {
        state: false,
        allow_toggle: AllowToggle::IfNamed,
    });
}

#[test]
fn test_locked_off_tool_choice_is_rejected() {
    use crate::{PartialAppConfig, assistant::tool_choice::ToolChoice, util::build};

    let mut partial = PartialAppConfig::new_test();
    partial.assistant.tool_choice = Some(ToolChoice::Function("net".to_owned()));
    partial
        .conversation
        .tools
        .tools
        .insert("net".to_owned(), PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            enable: Some(PartialEnableConfig::LOCKED_OFF),
            ..Default::default()
        });

    let err = build(partial).unwrap_err().to_string();
    assert!(
        err.contains("locked off") && err.contains("assistant.tool_choice"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_forced_tool_choice_on_toggleable_tool_is_accepted() {
    use crate::{PartialAppConfig, assistant::tool_choice::ToolChoice, util::build};

    // A forced tool that is merely disabled (not locked-off) is allowed.
    let mut partial = PartialAppConfig::new_test();
    partial.assistant.tool_choice = Some(ToolChoice::Function("net".to_owned()));
    partial
        .conversation
        .tools
        .tools
        .insert("net".to_owned(), PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            enable: Some(PartialEnableConfig::OFF),
            ..Default::default()
        });

    assert!(build(partial).is_ok());
}

#[test]
fn test_delta_enable_records_only_changed_subfield() {
    use crate::delta::PartialConfigDelta as _;

    let prev = PartialToolConfig {
        enable: Some(PartialEnableConfig {
            state: Some(true),
            allow_toggle: Some(AllowToggle::IfNamed),
        }),
        ..Default::default()
    };
    let next = PartialToolConfig {
        enable: Some(PartialEnableConfig {
            state: Some(false),
            allow_toggle: Some(AllowToggle::IfNamed),
        }),
        ..Default::default()
    };

    // Only `state` changed, so the delta carries just `state`.
    assert_eq!(
        prev.delta(next).enable,
        Some(PartialEnableConfig {
            state: Some(false),
            allow_toggle: None,
        })
    );
}
