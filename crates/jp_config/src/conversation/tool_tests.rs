use assert_matches::assert_matches;
use schematic::{PartialConfig as _, SchemaBuilder, SchemaType, schema::LiteralValue};

use super::*;

#[test]
fn test_enable_from_bool() {
    assert_eq!(Enable::from(true), Enable::On);
    assert_eq!(Enable::from(false), Enable::Off);
}

#[test]
fn test_enable_from_str() {
    assert_eq!("true".parse::<Enable>(), Ok(Enable::On));
    assert_eq!("on".parse::<Enable>(), Ok(Enable::On));
    assert_eq!("false".parse::<Enable>(), Ok(Enable::Off));
    assert_eq!("off".parse::<Enable>(), Ok(Enable::Off));
    assert_eq!("explicit".parse::<Enable>(), Ok(Enable::Explicit));
    assert_eq!("always".parse::<Enable>(), Ok(Enable::Always));
    assert!("invalid".parse::<Enable>().is_err());
}

#[test]
fn test_enable_serde_roundtrip() {
    // Serialize
    assert_eq!(serde_json::to_value(Enable::On).unwrap(), true);
    assert_eq!(serde_json::to_value(Enable::Off).unwrap(), false);
    assert_eq!(serde_json::to_value(Enable::Explicit).unwrap(), "explicit");
    assert_eq!(serde_json::to_value(Enable::Always).unwrap(), "always");

    // Deserialize from bool
    assert_eq!(
        serde_json::from_value::<Enable>(true.into()).unwrap(),
        Enable::On
    );
    assert_eq!(
        serde_json::from_value::<Enable>(false.into()).unwrap(),
        Enable::Off
    );

    // Deserialize from string
    assert_eq!(
        serde_json::from_value::<Enable>("on".into()).unwrap(),
        Enable::On
    );
    assert_eq!(
        serde_json::from_value::<Enable>("off".into()).unwrap(),
        Enable::Off
    );
    assert_eq!(
        serde_json::from_value::<Enable>("explicit".into()).unwrap(),
        Enable::Explicit
    );
    assert_eq!(
        serde_json::from_value::<Enable>("always".into()).unwrap(),
        Enable::Always
    );
}

#[test]
fn test_enable_assign_kv() {
    let mut p = PartialToolConfig::default_values(&()).unwrap().unwrap();

    // Assign via string "true"
    let kv = KvAssignment::try_from_cli("enable", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.enable, Some(Enable::On));

    // Assign via string "explicit"
    let kv = KvAssignment::try_from_cli("enable", "explicit").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.enable, Some(Enable::Explicit));

    // Assign via JSON bool
    let kv = KvAssignment::try_from_cli("enable:", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.enable, Some(Enable::Off));

    // Assign via JSON string
    let kv = KvAssignment::try_from_cli("enable:", r#""explicit""#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.enable, Some(Enable::Explicit));
}

#[test]
fn test_tools_config() {
    assert_matches!(PartialToolsConfig::default_values(&()), Ok(Some(_)));
    assert_matches!(PartialToolConfig::default_values(&()), Ok(Some(_)));

    let mut p = PartialToolsConfig::default_values(&()).unwrap().unwrap();

    p.tools.insert("cargo_check".to_owned(), PartialToolConfig {
        enable: Some(Enable::Off),
        source: Some(ToolSource::Local { tool: None }),
        ..Default::default()
    });

    let kv = KvAssignment::try_from_cli("cargo_check.enable", "true").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(
        p.tools,
        IndexMap::<_, _>::from_iter(vec![("cargo_check".to_owned(), PartialToolConfig {
            enable: Some(Enable::On),
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
                enable: Some(Enable::On),
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
    assert_eq!(cfg.command(), ToolCommandConfig {
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
        Some(PartialCommandConfigOrString::Config(
            PartialToolCommandConfig {
                program: Some("cargo".to_owned()),
                args: Some(vec!["check".to_owned(), "--verbose".to_owned()]),
                shell: Some(true),
            }
        ))
    );

    let cfg = CommandConfigOrString::from_partial(p.command.unwrap(), vec![]).unwrap();
    assert_eq!(cfg.command(), ToolCommandConfig {
        program: "cargo".to_owned(),
        args: vec!["check".to_owned(), "--verbose".to_owned()],
        shell: true,
    });
}

#[test]
fn test_enable_schema() {
    let schema = SchemaBuilder::build_root::<Enable>();
    assert_eq!(schema.name, Some("Enable".to_owned()));
    let SchemaType::Union(union) = schema.ty else {
        panic!("expected union")
    };
    assert_eq!(union.variants_types.len(), 2);
    assert_eq!(
        union.variants_types[0].ty,
        SchemaType::Boolean(Box::default())
    );
    let SchemaType::Enum(e) = &union.variants_types[1].ty else {
        panic!("expected enum")
    };
    assert_eq!(e.values, vec![
        LiteralValue::String("on".into()),
        LiteralValue::String("off".into()),
        LiteralValue::String("explicit".into()),
        LiteralValue::String("always".into())
    ]);
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
        enable: Some(Enable::Off),
        ..Default::default()
    });

    // Override only enable and run via a JSON object.
    let kv =
        KvAssignment::try_from_cli("my_tool:", r#"{"enable":true,"run":"unattended"}"#).unwrap();
    p.assign(kv).unwrap();

    let tool = p.tools.get("my_tool").unwrap();
    assert_eq!(tool.enable, Some(Enable::On));
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
        enable: Some(Enable::On),
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
        enable: Some(Enable::Off),
        ..Default::default()
    });

    // Nested object for style is forwarded to DisplayStyleConfig::assign.
    let kv =
        KvAssignment::try_from_cli("t:", r#"{"enable":true,"style":{"hidden":true}}"#).unwrap();
    p.assign(kv).unwrap();

    let tool = p.tools.get("t").unwrap();
    assert_eq!(tool.enable, Some(Enable::On));
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
