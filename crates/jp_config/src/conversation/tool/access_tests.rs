use super::*;

#[test]
fn deserializes_fs_rule_with_external_and_write_alias() {
    let rule: FsRuleConfig =
        serde_json::from_str(r#"{"path":"fork","external":true,"read":true,"write":true}"#)
            .unwrap();

    assert_eq!(rule.path, "fork");
    assert!(rule.is_external());
    assert_eq!(rule.read, Some(true));
    // The `write` alias expands to the three atomic write capabilities.
    assert!(rule.create() && rule.update() && rule.delete());
}

#[test]
fn explicit_capability_overrides_write_alias() {
    let rule: FsRuleConfig =
        serde_json::from_str(r#"{"path":"x","write":true,"delete":false}"#).unwrap();

    assert!(rule.create() && rule.update());
    assert!(!rule.delete());
}

#[test]
fn external_defaults_to_false() {
    let rule: FsRuleConfig = serde_json::from_str(r#"{"path":"src","read":true}"#).unwrap();
    assert!(!rule.is_external());
}

#[test]
fn to_partial_round_trips_rules() {
    let config = AccessConfig {
        fs: vec![
            FsRuleConfig {
                path: ".".to_owned(),
                external: None,
                read: Some(true),
                write: None,
                create: None,
                update: None,
                delete: None,
                execute: None,
            },
            FsRuleConfig {
                path: "fork".to_owned(),
                external: Some(true),
                read: Some(true),
                write: Some(true),
                create: None,
                update: None,
                delete: None,
                execute: None,
            },
        ],
    };

    let partial = config.to_partial();
    assert_eq!(partial.fs.len(), 2);
    assert_eq!(partial.fs[1].path.as_deref(), Some("fork"));
    assert_eq!(partial.fs[1].external, Some(true));
}
