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
    assert_matches!(error, Error::Schematic(MissingRequired{ fields }) if fields == vec!["conversation", "tools", "*", "run"]);
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

/// The inverse of `find_file_in_load_path`: every segment it could resolve.
///
/// Nested directories are part of the segment, since that is what selects the
/// file, and non-config files are not selectable at all.
#[test]
fn test_list_configs_in_load_path() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    write_config(&root.join("default.toml"), "");
    write_config(&root.join("skill/rfd.toml"), "");
    write_config(&root.join("skill/web.yaml"), "");
    write_config(&root.join("persona/deep/nested.json"), "");

    // Neither is a configuration file, so neither is selectable.
    fs::write(root.join("README.md"), "").unwrap();
    fs::write(root.join("skill/notes.txt"), "").unwrap();

    assert_eq!(list_configs_in_load_path(&root), vec![
        "default".to_owned(),
        "persona/deep/nested".to_owned(),
        "skill/rfd".to_owned(),
        "skill/web".to_owned(),
    ]);
}

/// A load path that isn't there holds nothing, which is not an error: a
/// workspace need not have every directory the load path names.
#[test]
fn test_list_configs_in_missing_load_path() {
    let tmp = tempdir().unwrap();

    assert!(list_configs_in_load_path(&tmp.path().join("absent")).is_empty());
}

/// Each listed segment resolves back to its own file, which is the contract
/// that makes a listed segment usable as `--cfg`.
///
/// A dotted stem is the case a substituting lookup gets wrong: `review.v2` has
/// to find `review.v2.toml` rather than the `review.toml` next to it, and
/// `model/gpt-4.1` has to resolve at all.
#[test]
fn test_listed_segments_resolve_back_to_their_files() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    write_config(&root.join("default.toml"), "");
    write_config(&root.join("skill/rfd.toml"), "");
    write_config(&root.join("review.toml"), "");
    write_config(&root.join("review.v2.toml"), "");
    write_config(&root.join("model/gpt-4.1.toml"), "");

    let want = [
        ("default", "default.toml"),
        ("model/gpt-4.1", "model/gpt-4.1.toml"),
        ("review", "review.toml"),
        ("review.v2", "review.v2.toml"),
        ("skill/rfd", "skill/rfd.toml"),
    ];

    assert_eq!(
        list_configs_in_load_path(&root),
        want.iter()
            .map(|(s, _)| (*s).to_owned())
            .collect::<Vec<_>>()
    );

    for (segment, file) in want {
        assert_eq!(
            find_file_in_load_path(&segment, &root),
            Some(root.join(file).into_std_path_buf()),
            "`{segment}` did not resolve to `{file}`"
        );
    }
}

/// A name carrying an extension that matches no file still finds a same-named
/// configuration, which is how `--cfg persona/dev.yaml` has always found
/// `persona/dev.toml`.
#[test]
fn test_find_file_in_load_path_substitutes_an_unmatched_extension() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    write_config(&root.join("persona/dev.toml"), "");

    assert_eq!(
        find_file_in_load_path(&"persona/dev.yaml", &root),
        Some(root.join("persona/dev.toml").into_std_path_buf())
    );
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

    let partial = load_partial_at_path(root.join("a.toml")).unwrap().unwrap();
    assert_eq!(partial.assistant.system_prompt.as_deref(), Some("d"));
}

#[test]
fn test_load_loader_directives_reads_own_section() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(
        &root.join("entry.toml"),
        indoc::indoc!(
            r#"
                [loader]
                reset = "none"
            "#
        ),
    );

    let loader = load_loader_directives(root.join("entry.toml")).unwrap();
    assert_eq!(loader.reset, Some(crate::loader::LoaderReset::None));

    write_config(&root.join("plain.toml"), "assistant.system_prompt = \"p\"");
    let loader = load_loader_directives(root.join("plain.toml")).unwrap();
    assert_eq!(loader.reset, None);
}

#[test]
fn test_load_loader_directives_ignores_extends() {
    // The read is shallow: `[loader]` in a file reached through `extends`
    // must not affect the declaring entry ([RFD 038]).
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(
        &root.join("entry.toml"),
        indoc::indoc!(
            r#"
                extends = ["fragment.toml"]
            "#
        ),
    );
    write_config(
        &root.join("fragment.toml"),
        indoc::indoc!(
            r#"
                [loader]
                reset = "none"
            "#
        ),
    );

    let loader = load_loader_directives(root.join("entry.toml")).unwrap();
    assert_eq!(loader.reset, None);

    // The full load, by contrast, merges the fragment's section into the
    // resolved partial; the pipeline strips it after reading directives.
    let partial = load_partial_at_path(root.join("entry.toml"))
        .unwrap()
        .unwrap();
    assert_eq!(partial.loader.reset, Some(crate::loader::LoaderReset::None));
}

#[test]
fn test_load_partial_at_path_diamond_applies_shared_file_once() {
    // a -> b -> d
    // a -> c -> d
    //
    // `d` appends to the system prompt. Reaching it through both branches must
    // apply the append once: a shared dependency is a diamond, not two
    // separate contributions.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(&root.join("a.toml"), r#"extends = ["b.toml", "c.toml"]"#);
    write_config(&root.join("b.toml"), r#"extends = ["d.toml"]"#);
    write_config(&root.join("c.toml"), r#"extends = ["d.toml"]"#);
    write_config(
        &root.join("d.toml"),
        indoc::indoc!(
            r#"
                [assistant.system_prompt]
                strategy = "append"
                separator = "space"
                value = "d"
            "#
        ),
    );

    let partial = load_partial_at_path(root.join("a.toml")).unwrap().unwrap();
    assert_eq!(partial.assistant.system_prompt.as_deref(), Some("d"));
}

#[test]
fn test_load_partial_at_path_diamond_keeps_shared_file_last() {
    // a -> b -> d
    // a -> c -> d
    //
    // `b` and `d` both set the same replace-merged field. Collapsing `d`'s two
    // visits to the last one keeps `d` after `b`, so `d` wins — which is the
    // same winner the uncollapsed graph produced (`[d, b, d, c, a]`), because
    // the repeat visit already clobbered `b`.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(&root.join("a.toml"), r#"extends = ["b.toml", "c.toml"]"#);
    write_config(
        &root.join("b.toml"),
        indoc::indoc!(
            r#"
                extends = ["d.toml"]
                assistant.name = "b"
            "#
        ),
    );
    write_config(&root.join("c.toml"), r#"extends = ["d.toml"]"#);
    write_config(&root.join("d.toml"), r#"assistant.name = "d""#);

    let partial = load_partial_at_path(root.join("a.toml")).unwrap().unwrap();
    assert_eq!(partial.assistant.name.as_deref(), Some("d"));
}

#[test]
fn test_load_partial_at_path_repeat_visit_keeps_last_position() {
    // a -> b -> d
    // a -> d
    //
    // `a` declares `extends = ["b.toml", "d.toml"]`, so `d` overrides `b`.
    // Collapsing the repeat visit must not move `d` ahead of `b` and flip that
    // precedence.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(&root.join("a.toml"), r#"extends = ["b.toml", "d.toml"]"#);
    write_config(
        &root.join("b.toml"),
        indoc::indoc!(
            r#"
                extends = ["d.toml"]
                assistant.name = "b"
            "#
        ),
    );
    write_config(&root.join("d.toml"), r#"assistant.name = "d""#);

    let partial = load_partial_at_path(root.join("a.toml")).unwrap().unwrap();
    assert_eq!(partial.assistant.name.as_deref(), Some("d"));
}

/// The `config_load_paths` entries of a loaded partial, as strings.
fn load_paths(partial: &PartialAppConfig) -> Vec<&str> {
    partial
        .config_load_paths
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|p| p.as_str())
        .collect()
}

#[test]
fn test_load_partial_at_path_dedups_load_paths_across_files() {
    // Two files naming the same search directory contribute it once. This is
    // the path the merge strategy actually runs on: `merge_setting` only
    // invokes it when both layers supply a value.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(
        &root.join("a.toml"),
        indoc::indoc!(
            r#"
                extends = ["b.toml"]
                config_load_paths = ["shared", "a-only"]
            "#
        ),
    );
    write_config(
        &root.join("b.toml"),
        r#"config_load_paths = ["shared", "b-only"]"#,
    );

    let partial = load_partial_at_path(root.join("a.toml")).unwrap().unwrap();

    assert_eq!(load_paths(&partial), ["shared", "b-only", "a-only"]);
}

#[test]
fn test_load_partial_at_path_keeps_repeats_from_a_single_file() {
    // One file, so nothing is combined and the list is stored as written.
    // Repeats inside a single source are the author's own data, the same rule
    // `replace` follows on `MergeableVec`; the resolved list is searched in
    // order and stops at the first match, so a repeat changes no outcome.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    write_config(
        &root.join("a.toml"),
        r#"config_load_paths = ["dupe", "dupe"]"#,
    );

    let partial = load_partial_at_path(root.join("a.toml")).unwrap().unwrap();

    assert_eq!(load_paths(&partial), ["dupe", "dupe"]);
}
