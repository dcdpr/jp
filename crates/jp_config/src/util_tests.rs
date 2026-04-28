use std::fs;

use assert_matches::assert_matches;
use camino_tempfile::tempdir;
use serde_json::{Value, json};
use serial_test::serial;
use test_log::test;

use super::*;
use crate::{
    assistant::instructions::PartialInstructionsConfig,
    conversation::tool::RunMode,
    model::id::{PartialModelIdConfig, ProviderId},
    types::vec::{MergedVec, MergedVecStrategy},
};

// Helper to write config content to a file, creating parent dirs
fn write_config(path: &Utf8Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn test_load_partials_with_inheritance() {
    struct TestCase {
        partials: Vec<PartialAppConfig>,
        want: (&'static str, Option<Value>),
    }

    let cases = vec![
        ("disabled inheritance", TestCase {
            partials: vec![
                {
                    let mut partial = PartialAppConfig::empty();
                    partial.providers.llm.openrouter.api_key_env = Some("FOO".to_owned());
                    partial
                },
                {
                    let mut partial = PartialAppConfig::empty();
                    partial.providers.llm.openrouter.api_key_env = Some("BAR".to_owned());
                    partial.inherit = Some(false);
                    partial
                },
                {
                    let mut partial = PartialAppConfig::empty();
                    partial.providers.llm.openrouter.api_key_env = Some("BAZ".to_owned());
                    partial
                },
            ],
            want: ("/providers/llm/openrouter/api_key_env", Some("BAR".into())),
        }),
        ("inheritance", TestCase {
            partials: vec![
                {
                    let mut partial = PartialAppConfig::empty();
                    partial.providers.llm.openrouter.api_key_env = Some("FOO".to_owned());
                    partial
                },
                {
                    let mut partial = PartialAppConfig::empty();
                    partial.providers.llm.openrouter.api_key_env = Some("BAR".to_owned());
                    partial.inherit = Some(true);
                    partial
                },
                {
                    let mut partial = PartialAppConfig::empty();
                    partial.providers.llm.openrouter.api_key_env = Some("BAZ".to_owned());
                    partial
                },
            ],
            want: ("/providers/llm/openrouter/api_key_env", Some("BAZ".into())),
        }),
    ];

    for (name, case) in cases {
        let partial = load_partials_with_inheritance(case.partials).unwrap();
        let json = serde_json::to_value(&partial).unwrap();
        let val = json.pointer(case.want.0);

        assert_eq!(val, case.want.1.as_ref(), "failed case: {name}");
    }
}

#[test]
#[serial(env_vars)]
fn test_load_envs() {
    let _env = EnvVarGuard::set("JP_CFG_PROVIDERS_LLM_OPENROUTER_API_KEY_ENV", "ENV1");

    let partial = load_envs(PartialAppConfig::empty()).unwrap();
    assert_eq!(
        partial.providers.llm.openrouter.api_key_env,
        Some("ENV1".to_owned())
    );
}

#[test]
#[serial(env_vars)]
fn test_load_envs_overrides_file_config() {
    let _env = EnvVarGuard::set("JP_CFG_PROVIDERS_LLM_OPENROUTER_API_KEY_ENV", "FROM_ENV");

    let mut file_config = PartialAppConfig::empty();
    file_config.providers.llm.openrouter.api_key_env = Some("FROM_FILE".to_owned());

    let merged = load_envs(file_config).unwrap();
    assert_eq!(
        merged.providers.llm.openrouter.api_key_env,
        Some("FROM_ENV".to_owned()),
        "environment variables should override file config"
    );
}

#[test]
fn test_build() {
    let error = build(PartialAppConfig::default_values(&()).unwrap().unwrap()).unwrap_err();
    assert_matches!(
        error,
        Error::Schematic(schematic::ConfigError::MissingRequired { .. })
    );

    let mut partial = PartialAppConfig::default_values(&()).unwrap().unwrap();
    partial.assistant.model.id = PartialModelIdConfig {
        provider: Some(ProviderId::Openrouter),
        name: Some("foo".parse().unwrap()),
    }
    .into();

    partial.conversation.tools.defaults.run = Some(RunMode::Unattended);

    let config = build(partial).unwrap();
    assert_eq!(
        config.providers.llm.openrouter.api_key_env,
        "OPENROUTER_API_KEY".to_owned()
    );
}

#[test]
fn test_build_without_required_fields() {
    use schematic::ConfigError::MissingRequired;

    let mut partial = PartialAppConfig::default_values(&()).unwrap().unwrap();

    let error = build(partial.clone()).unwrap_err();
    assert_matches!(error, Error::Schematic(MissingRequired { fields }) if fields == vec!["assistant", "model", "id", "provider"]);
    partial.assistant.model.id = PartialModelIdConfig {
        provider: Some(ProviderId::Openrouter),
        name: Some("foo".parse().unwrap()),
    }
    .into();

    let error = build(partial.clone()).unwrap_err();
    assert_matches!(error, Error::Schematic(MissingRequired{ fields }) if fields == vec!["conversation", "tools", "defaults", "run"]);
    partial.conversation.tools.defaults.run = Some(RunMode::Unattended);

    build(partial).unwrap();
}

#[test]
fn test_build_sorted_instructions() {
    let mut partial = PartialAppConfig::empty();
    partial.conversation.tools.defaults.run = Some(RunMode::Unattended);
    partial.assistant.model.id = PartialModelIdConfig {
        provider: Some(ProviderId::Openrouter),
        name: Some("foo".parse().unwrap()),
    }
    .into();
    partial.assistant.instructions = MergedVec {
        value: vec![
            PartialInstructionsConfig {
                title: None,
                description: None,
                position: Some(100),
                items: Some(vec![]),
                examples: vec![],
            },
            PartialInstructionsConfig {
                title: None,
                description: None,
                position: Some(-1),
                items: Some(vec![]),
                examples: vec![],
            },
            PartialInstructionsConfig {
                title: None,
                description: None,
                position: Some(0),
                items: Some(vec![]),
                examples: vec![],
            },
        ],
        strategy: Some(MergedVecStrategy::Replace),
        dedup: None,
        discard_when_merged: false,
    }
    .into();

    let config = build(partial).unwrap();

    assert_eq!(config.assistant.instructions[0].position, -1);
    assert_eq!(config.assistant.instructions[1].position, 0);
    assert_eq!(config.assistant.instructions[2].position, 100);
}

#[test]
fn test_load_partial_at_path() {
    struct TestCase {
        file: &'static str,
        data: &'static str,
        arg: &'static str,
        want: Result<Option<&'static str>, &'static str>,
    }

    let cases = vec![
        ("exact match toml", TestCase {
            file: "config.toml",
            data: "providers.llm.openrouter.api_key_env = 'FOO'",
            arg: "config.toml",
            want: Ok(Some("FOO")),
        }),
        ("exact match json", TestCase {
            file: "config.json",
            data: r#"{"providers":{"llm":{"openrouter":{"api_key_env":"FOO"}}}}"#,
            arg: "config.json",
            want: Ok(Some("FOO")),
        }),
        ("exact match yaml", TestCase {
            file: "config.yaml",
            data: "providers:\n  llm:\n    openrouter:\n      api_key_env: FOO",
            arg: "config.yaml",
            want: Ok(Some("FOO")),
        }),
        ("toml mismatch", TestCase {
            file: "config.toml",
            data: "providers.llm.openrouter.api_key_env = 'FOO'",
            arg: "config.json",
            want: Ok(Some("FOO")),
        }),
        ("json mismatch", TestCase {
            file: "config.json",
            data: r#"{"providers":{"llm":{"openrouter":{"api_key_env":"FOO"}}}}"#,
            arg: "config.yaml",
            want: Ok(Some("FOO")),
        }),
        ("yaml mismatch", TestCase {
            file: "config.yaml",
            data: "providers:\n  llm:\n    openrouter:\n      api_key_env: FOO",
            arg: "config.toml",
            want: Ok(Some("FOO")),
        }),
        ("no extension", TestCase {
            file: "config.toml",
            data: "providers.llm.openrouter.api_key_env = 'FOO'",
            arg: "config",
            want: Ok(Some("FOO")),
        }),
        ("no match", TestCase {
            file: "config.ini",
            data: "",
            arg: "config.toml",
            want: Ok(None),
        }),
        ("found invalid file", TestCase {
            file: "config.ini",
            data: "",
            arg: "config.ini",
            want: Err("no matching source format for extension ini"),
        }),
    ];

    for (name, case) in cases {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_config(&root.join(case.file), case.data);

        let partial = load_partial_at_path(root.join(case.arg));
        if let Err(err) = &case.want {
            assert!(partial.is_err(), "failed case: {name}");
            let actual = partial.unwrap_err().to_string();
            assert!(
                actual.contains(err),
                "failed case: {name}, expected error '{actual}' to contain '{err}'"
            );
            continue;
        }

        assert_eq!(
            partial
                .map(|r| r.and_then(|p| p.providers.llm.openrouter.api_key_env))
                .map_err(|e| e.to_string()),
            case.want
                .map(|v| v.map(str::to_owned))
                .map_err(str::to_owned),
            "failed case: {name}",
        );
    }
}

#[test]
fn test_load_partial_at_path_recursive() {
    struct TestCase {
        files: Vec<(&'static str, &'static str)>,
        path: &'static str,
        root: Option<&'static str>,
        want: Result<Option<(&'static str, Option<Value>)>, &'static str>,
    }

    let cases = vec![
        ("override from longest path", TestCase {
            files: vec![
                (
                    "foo/config.toml",
                    "providers.llm.openrouter.api_key_env = 'FOO'",
                ),
                (
                    "config.json",
                    r#"{"providers":{"llm":{"openrouter":{"api_key_env":"BAR"}}}}"#,
                ),
            ],
            path: "foo/config.toml",
            root: None,
            want: Ok(Some((
                "/providers/llm/openrouter/api_key_env",
                Some("FOO".into()),
            ))),
        }),
        ("merge different paths", TestCase {
            files: vec![
                (
                    "foo/config.toml",
                    "providers.llm.openrouter.api_key_env = 'FOO'",
                ),
                (
                    "config.json",
                    r#"{"providers":{"llm":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                ),
            ],
            path: "foo/config.toml",
            root: None,
            want: Ok(Some((
                "/providers/llm/openrouter",
                Some(json!({"api_key_env": "FOO", "app_referrer": "BAR"})),
            ))),
        }),
        ("find upstream", TestCase {
            files: vec![
                (
                    "foo/config.toml",
                    "providers.llm.openrouter.api_key_env = 'FOO'",
                ),
                (
                    "config.json",
                    r#"{"providers":{"llm":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                ),
            ],
            path: "foo/bar/baz/config.yaml",
            root: None,
            want: Ok(Some((
                "/providers/llm/openrouter",
                Some(json!({"api_key_env": "FOO", "app_referrer": "BAR"})),
            ))),
        }),
        ("merge until root", TestCase {
            files: vec![
                (
                    "foo/config.toml",
                    "providers.llm.openrouter.api_key_env = 'FOO'",
                ),
                (
                    "config.json",
                    r#"{"providers":{"llm":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                ),
            ],
            path: "foo/bar/config.yaml",
            root: Some("foo"),
            want: Ok(Some((
                "/providers/llm/openrouter",
                Some(json!({"api_key_env": "FOO"})),
            ))),
        }),
        ("load dir instead of file", TestCase {
            files: vec![
                (
                    "foo/config.toml",
                    "providers.llm.openrouter.api_key_env = 'FOO'",
                ),
                (
                    "config.json",
                    r#"{"providers":{"llm":{"openrouter":{"app_referrer":"BAR"}}}}"#,
                ),
            ],
            path: "foo",
            root: Some(""),
            want: Ok(None),
        }),
        ("regular extends with string replace", TestCase {
            files: vec![
                (
                    // loaded first, merged last
                    "config.toml",
                    indoc::indoc!(
                        r#"
                            extends = ["one.toml", "two.toml"]
                            assistant.system_prompt = "foo"
                        "#
                    ),
                ),
                (
                    // loaded second, merged first
                    "one.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = "bar"
                        "#
                    ),
                ),
                (
                    // loaded third, merged second
                    "two.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = "baz"
                        "#
                    ),
                ),
            ],
            path: "config.toml",
            root: None,
            want: Ok(Some(("/assistant/system_prompt", Some("foo".into())))),
        }),
        ("regular extends with merged string", TestCase {
            files: vec![
                (
                    // loaded first, merged last
                    "config.toml",
                    indoc::indoc!(
                        r#"
                            extends = ["one.toml", "two.toml"]
                            assistant.system_prompt = { value = "foo", strategy = "prepend" }
                        "#
                    ),
                ),
                (
                    // loaded second, merged first
                    "one.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = "baz"
                        "#
                    ),
                ),
                (
                    // loaded third, merged second
                    "two.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = { value = "bar", strategy = "prepend" }
                        "#
                    ),
                ),
            ],
            path: "config.toml",
            root: None,
            want: Ok(Some((
                "/assistant/system_prompt",
                Some(json!({ "value": "foobarbaz", "strategy": "prepend" })),
            ))),
        }),
        ("nested extends with merged string", TestCase {
            files: vec![
                (
                    // loaded first, merged last
                    "config.toml",
                    indoc::indoc!(
                        r#"
                            extends = ["one.toml", "three.toml"]
                            assistant.system_prompt = { value = "foo", strategy = "prepend" }
                        "#
                    ),
                ),
                (
                    // loaded second, merged second
                    "one.toml",
                    indoc::indoc!(
                        r#"
                            extends = [{ path = "two.toml", strategy = "after" }]
                            assistant.system_prompt = "baz"
                        "#
                    ),
                ),
                (
                    // loaded third, merged first
                    "two.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = { value = "qux", strategy = "append" }
                        "#
                    ),
                ),
                (
                    // loaded fourth, merged third
                    "three.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = { value = "bar", strategy = "prepend" }
                        "#
                    ),
                ),
            ],
            path: "config.toml",
            root: None,
            want: Ok(Some((
                "/assistant/system_prompt",
                Some(json!({ "value": "foobarbazqux", "strategy": "prepend" })),
            ))),
        }),
        ("complex extends", TestCase {
            files: vec![
                (
                    // loaded first, merged fourth
                    "config.toml",
                    indoc::indoc!(
                        r#"
                            extends = [
                                "one.toml",
                                { path = "two.toml", strategy = "before" },
                                { path = "three.toml", strategy = "after" },
                            ]

                            assistant.system_prompt = { value = "foo", strategy = "prepend" }
                        "#
                    ),
                ),
                (
                    // loaded second, merged second
                    "one.toml",
                    indoc::indoc!(
                        r#"
                            extends = [{ path = "four.toml", strategy = "before" }]

                            assistant.system_prompt = { value = "bar", strategy = "append" }
                        "#
                    ),
                ),
                (
                    // loaded fourth, merged third
                    "two.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = { value = "baz", strategy = "append" }
                        "#
                    ),
                ),
                (
                    // loaded fifth, merged last
                    "three.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = { value = "qux", strategy = "append" }
                        "#
                    ),
                ),
                (
                    // loaded third, merged first
                    "four.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = { value = "quux", strategy = "replace" }
                        "#
                    ),
                ),
                (
                    // ignored
                    "five.toml",
                    indoc::indoc!(
                        r#"
                            assistant.system_prompt = { value = "ignored", strategy = "replace" }
                        "#
                    ),
                ),
            ],
            path: "config.toml",
            root: None,
            want: Ok(Some((
                "/assistant/system_prompt",
                Some(json!({"value": "fooquuxbarbazqux", "strategy": "append"})),
            ))),
        }),
    ];

    for (name, case) in cases {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for (file, data) in case.files {
            write_config(&root.join(file), data);
        }
        let root_arg = case.root.map(|r| root.join(r));

        let got = load_partial_at_path_recursive(root.join(case.path), root_arg.as_deref());

        match (got, case.want) {
            (Err(got), Err(want)) => assert_eq!(got.to_string(), want.to_owned()),
            (Ok(None), Ok(None)) => {}
            (Ok(Some(got)), Ok(Some((path, want)))) => {
                let json = serde_json::to_value(&got).unwrap();
                let val = json.pointer(path);
                assert_eq!(val, want.as_ref(), "failed case: {name}");
            }
            (got, want) => {
                panic!("failed case: {name}\n\ngot:  {got:?}\nwant: {want:?}")
            }
        }
    }
}

#[test]
fn test_load_partial_at_path_self_extending_cycle() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(
        &root.join("config.toml"),
        indoc::indoc!(
            r#"
                extends = ["config.toml"]
            "#
        ),
    );

    let err = load_partial_at_path(root.join("config.toml")).unwrap_err();
    assert_matches!(err, Error::ExtendsCycle { chain } if chain.len() == 2);
}

#[test]
fn test_load_partial_at_path_two_node_cycle() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(
        &root.join("a.toml"),
        indoc::indoc!(
            r#"
                extends = ["b.toml"]
            "#
        ),
    );
    write_config(
        &root.join("b.toml"),
        indoc::indoc!(
            r#"
                extends = ["a.toml"]
            "#
        ),
    );

    let err = load_partial_at_path(root.join("a.toml")).unwrap_err();
    assert_matches!(err, Error::ExtendsCycle { chain } if chain.len() == 3);
}

#[test]
fn test_load_partial_at_path_depth_cap() {
    // Four-file linear chain a -> b -> c -> d. With max_depth = 3, pushing the
    // 4th file (d) exceeds the cap and must return `ExtendsDepthExceeded`.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(
        &root.join("a.toml"),
        indoc::indoc!(
            r#"
                extends = ["b.toml"]
            "#
        ),
    );
    write_config(
        &root.join("b.toml"),
        indoc::indoc!(
            r#"
                extends = ["c.toml"]
            "#
        ),
    );
    write_config(
        &root.join("c.toml"),
        indoc::indoc!(
            r#"
                extends = ["d.toml"]
            "#
        ),
    );
    write_config(&root.join("d.toml"), "");

    let err = load_partial_at_path_with_max_depth(root.join("a.toml"), 3).unwrap_err();
    assert_matches!(
        err,
        Error::ExtendsDepthExceeded { limit: 3, chain } if chain.len() == 4
    );

    // With the cap raised to 4, the same chain loads cleanly.
    load_partial_at_path_with_max_depth(root.join("a.toml"), 4).unwrap();
}

#[test]
fn test_load_partial_at_path_diamond_is_not_a_cycle() {
    // a -> b -> d
    // a -> c -> d
    //
    // `d` appears twice in the overall load graph but never re-enters the
    // ancestor chain, so this must succeed.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(
        &root.join("a.toml"),
        indoc::indoc!(
            r#"
                extends = ["b.toml", "c.toml"]
            "#
        ),
    );
    write_config(
        &root.join("b.toml"),
        indoc::indoc!(
            r#"
                extends = ["d.toml"]
            "#
        ),
    );
    write_config(
        &root.join("c.toml"),
        indoc::indoc!(
            r#"
                extends = ["d.toml"]
            "#
        ),
    );
    write_config(&root.join("d.toml"), "assistant.system_prompt = \"d\"");

    let partial = load_partial_at_path(root.join("a.toml")).unwrap();
    assert!(partial.is_some());
}

#[test]
fn test_vec_dedup_preserves_order() {
    let result = vec_dedup(vec![3, 1, 2, 1, 3, 4], &()).unwrap();
    assert_eq!(result, vec![3, 1, 2, 4]);
}

#[test]
fn test_vec_dedup_no_duplicates() {
    let result = vec_dedup(vec![1, 2, 3], &()).unwrap();
    assert_eq!(result, vec![1, 2, 3]);
}
