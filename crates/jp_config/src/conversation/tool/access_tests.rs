use super::*;

fn env_rule(name: &str, read: bool) -> EnvRuleConfig {
    EnvRuleConfig {
        name: name.to_owned(),
        read: Some(read),
    }
}

fn fs_rule(path: &str) -> FsRuleConfig {
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

/// Fold a delta back over the state it was computed against, as the event
/// stream does on the next invocation.
fn fold(prev: &PartialAccessConfig, next: PartialAccessConfig) -> PartialAccessConfig {
    use schematic::PartialConfig as _;

    let delta = prev.delta(next);

    let mut folded = prev.clone();
    folded.merge(&(), delta).unwrap();
    folded
}

fn env_names(config: &PartialAccessConfig) -> Vec<&str> {
    config
        .env
        .iter()
        .filter_map(|rule| rule.name.as_deref())
        .collect()
}

fn fs_paths(config: &PartialAccessConfig) -> Vec<&str> {
    config
        .fs
        .iter()
        .filter_map(|rule| rule.path.as_deref())
        .collect()
}

/// A `--cfg` layer that narrows a tool's env grants must not have its removal
/// undone by the fold.
/// An append-shaped delta can only add, so it would re-grant the credential the
/// layer dropped.
#[test]
fn an_env_delta_carries_a_removal_through_the_fold() {
    let prev = AccessConfig {
        fs: vec![],
        env: vec![env_rule("AWS_*", true), env_rule("CI", true)],
    }
    .to_partial();
    let next = AccessConfig {
        fs: vec![],
        env: vec![env_rule("CI", true)],
    }
    .to_partial();

    assert_eq!(env_names(&fold(&prev, next)), vec!["CI"]);
}

#[test]
fn an_env_delta_still_appends_when_nothing_is_removed() {
    let prev = AccessConfig {
        fs: vec![],
        env: vec![env_rule("AWS_*", true)],
    }
    .to_partial();
    let next = AccessConfig {
        fs: vec![],
        env: vec![env_rule("AWS_*", true), env_rule("CI", true)],
    }
    .to_partial();

    assert_eq!(env_names(&fold(&prev, next)), vec!["AWS_*", "CI"]);
}

#[test]
fn an_fs_delta_carries_a_removal_through_the_fold() {
    let prev = AccessConfig {
        fs: vec![fs_rule("src"), fs_rule("docs")],
        env: vec![],
    }
    .to_partial();
    let next = AccessConfig {
        fs: vec![fs_rule("src")],
        env: vec![],
    }
    .to_partial();

    assert_eq!(fs_paths(&fold(&prev, next)), vec!["src"]);
}

#[test]
fn an_fs_delta_still_appends_when_nothing_is_removed() {
    let prev = AccessConfig {
        fs: vec![fs_rule("src")],
        env: vec![],
    }
    .to_partial();
    let next = AccessConfig {
        fs: vec![fs_rule("src"), fs_rule("docs")],
        env: vec![],
    }
    .to_partial();

    assert_eq!(fs_paths(&fold(&prev, next)), vec!["src", "docs"]);
}

/// Each axis diffs on its own: narrowing `env` must not drag `fs` into replace
/// semantics, or a tool would lose path grants it never touched.
#[test]
fn narrowing_one_axis_leaves_the_other_alone() {
    let prev = AccessConfig {
        fs: vec![fs_rule("src")],
        env: vec![env_rule("AWS_*", true), env_rule("CI", true)],
    }
    .to_partial();
    let next = AccessConfig {
        fs: vec![fs_rule("src"), fs_rule("docs")],
        env: vec![env_rule("CI", true)],
    }
    .to_partial();

    let folded = fold(&prev, next);

    assert_eq!(env_names(&folded), vec!["CI"]);
    assert_eq!(fs_paths(&folded), vec!["src", "docs"]);
}

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
        env: vec![EnvRuleConfig {
            name: "AWS_*".to_owned(),
            read: Some(true),
        }],
    };

    let partial = config.to_partial();
    assert_eq!(partial.fs.len(), 2);
    assert_eq!(partial.fs[1].path.as_deref(), Some("fork"));
    assert_eq!(partial.fs[1].external, Some(true));
    assert_eq!(partial.env.len(), 1);
    assert_eq!(partial.env[0].name.as_deref(), Some("AWS_*"));
    assert_eq!(partial.env[0].read, Some(true));
}

#[test]
fn deserializes_env_rule() {
    let rule: EnvRuleConfig = serde_json::from_str(r#"{"name":"AWS_*","read":true}"#).unwrap();

    assert_eq!(rule.name, "AWS_*");
    assert!(rule.read());
}

#[test]
fn an_env_rule_without_read_denies() {
    let rule: EnvRuleConfig = serde_json::from_str(r#"{"name":"HOME"}"#).unwrap();

    assert_eq!(rule.read, None);
    assert!(!rule.read());
}
