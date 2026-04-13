use std::fs;

use camino::Utf8PathBuf;
use camino_tempfile::tempdir;
use clap::CommandFactory;
use jp_config::PartialAppConfig;
use jp_workspace::Workspace;
use relative_path::RelativePathBuf;
use serial_test::serial;
use test_log::test;

use super::*;

fn write_config(path: &camino::Utf8Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn partial_with_load_paths(paths: &[&str]) -> PartialAppConfig {
    let mut partial = PartialAppConfig::empty();
    partial.config_load_paths = Some(paths.iter().map(|p| RelativePathBuf::from(*p)).collect());
    partial
}

/// Helper: build a pipeline from a base partial + overrides, then return the
/// built partial (without conversation layer).
fn build_cfg(
    base: PartialAppConfig,
    overrides: &[KeyValueOrPath],
    workspace: Option<&Workspace>,
) -> Result<PartialAppConfig> {
    let pipeline = config_pipeline::ConfigPipeline::new(base, overrides, workspace)?;
    pipeline.partial_without_conversation()
}

#[test]
fn test_cli() {
    Cli::command().debug_assert();
}

#[test]
fn test_load_cli_cfg_args_workspace_root() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let workspace = Workspace::new(root);

    write_config(
        &root.join(".jp/config/skill/web.toml"),
        "assistant.name = 'from-workspace'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, Some(&workspace)).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-workspace"));
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_user_global_root() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_FILE", global_dir.as_str()) };

    write_config(
        &global_dir.join("config/.jp/config/skill/web.toml"),
        "assistant.name = 'from-global'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-global"));

    unsafe { std::env::remove_var("JP_GLOBAL_CONFIG_FILE") };
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_merges_global_and_workspace() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let ws_root = tmp.path().join("workspace");

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_FILE", global_dir.as_str()) };

    let workspace = Workspace::new(&ws_root);

    write_config(
        &global_dir.join("config/.jp/config/skill/web.toml"),
        "assistant.name = 'from-global'",
    );
    write_config(
        &ws_root.join(".jp/config/skill/web.toml"),
        "providers.llm.openrouter.api_key_env = 'FROM_WS'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, Some(&workspace)).unwrap();

    assert_eq!(result.assistant.name.as_deref(), Some("from-global"));
    assert_eq!(
        result.providers.llm.openrouter.api_key_env.as_deref(),
        Some("FROM_WS")
    );

    unsafe { std::env::remove_var("JP_GLOBAL_CONFIG_FILE") };
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_workspace_overrides_global() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let ws_root = tmp.path().join("workspace");

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_FILE", global_dir.as_str()) };

    let workspace = Workspace::new(&ws_root);

    write_config(
        &global_dir.join("config/.jp/config/skill/web.toml"),
        "assistant.name = 'from-global'",
    );
    write_config(
        &ws_root.join(".jp/config/skill/web.toml"),
        "assistant.name = 'from-workspace'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, Some(&workspace)).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-workspace"));

    unsafe { std::env::remove_var("JP_GLOBAL_CONFIG_FILE") };
}

#[test]
fn test_load_cli_cfg_args_missing_file_reports_searched_paths() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let workspace = Workspace::new(root);

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/missing"))];

    let err = build_cfg(partial, &overrides, Some(&workspace)).unwrap_err();
    match err {
        Error::MissingConfigFile { path, searched } => {
            assert_eq!(path.as_str(), "skill/missing");
            assert!(
                searched
                    .iter()
                    .any(|p| p.as_str().replace('\\', "/").contains(".jp/config")),
                "Expected searched paths to contain workspace load path, got: {searched:?}"
            );
        }
        other => panic!("Expected MissingConfigFile, got: {other:?}"),
    }
}

#[test]
fn test_load_cli_cfg_args_first_load_path_wins_within_root() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let workspace = Workspace::new(root);

    write_config(
        &root.join("first/skill/web.toml"),
        "assistant.name = 'from-first'",
    );
    write_config(
        &root.join("second/skill/web.toml"),
        "assistant.name = 'from-second'",
    );

    let partial = partial_with_load_paths(&["first", "second"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, Some(&workspace)).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-first"));
}

#[test]
fn test_load_cli_cfg_args_absolute_path_still_works() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let file = root.join("direct.toml");
    write_config(&file, "assistant.name = 'direct'");

    let partial = PartialAppConfig::empty();
    let overrides = vec![KeyValueOrPath::Path(file)];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("direct"));
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_no_roots_errors() {
    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("foobar/baz"))];

    let err = build_cfg(partial, &overrides, None).unwrap_err();
    match err {
        Error::MissingConfigFile { path, .. } => {
            assert_eq!(path.as_str(), "foobar/baz");
        }
        other => panic!("Expected MissingConfigFile, got: {other:?}"),
    }
}

#[test]
fn test_load_cli_cfg_args_key_value_still_works() {
    let partial = PartialAppConfig::empty();
    let overrides = vec![KeyValueOrPath::from_str("assistant.name=test").unwrap()];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("test"));
}

#[test]
fn test_load_cli_cfg_json_object() {
    let partial = PartialAppConfig::empty();
    let overrides =
        vec![KeyValueOrPath::from_str(r#"{"assistant": {"name": "from-json"}}"#).unwrap()];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-json"));
}

#[test]
fn test_load_cli_cfg_json_nested_object() {
    let partial = PartialAppConfig::empty();
    let json = r#"{"conversation": {"start_local": true}}"#;
    let overrides = vec![KeyValueOrPath::from_str(json).unwrap()];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.conversation.start_local, Some(true));
}

#[test]
fn test_load_cli_cfg_json_combined_with_key_value() {
    let partial = PartialAppConfig::empty();
    let overrides = vec![
        KeyValueOrPath::from_str(r#"{"assistant": {"name": "json-name"}}"#).unwrap(),
        KeyValueOrPath::from_str("conversation.start_local=true").unwrap(),
    ];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("json-name"));
    assert_eq!(result.conversation.start_local, Some(true));
}

#[test]
fn test_load_cli_cfg_json_invalid_json_errors() {
    let result = KeyValueOrPath::from_str("{not valid json");
    assert!(result.is_err());
}

#[test]
fn test_load_cli_cfg_json_overrides_earlier_values() {
    let partial = PartialAppConfig::empty();
    let overrides = vec![
        KeyValueOrPath::from_str("assistant.name=first").unwrap(),
        KeyValueOrPath::from_str(r#"{"assistant": {"name": "second"}}"#).unwrap(),
    ];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("second"));
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_global_only_when_workspace_has_no_match() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let ws_root = tmp.path().join("workspace");

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_FILE", global_dir.as_str()) };

    let workspace = Workspace::new(&ws_root);

    write_config(
        &global_dir.join("config/.jp/config/skill/web.toml"),
        "assistant.name = 'from-global'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, Some(&workspace)).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-global"));

    unsafe { std::env::remove_var("JP_GLOBAL_CONFIG_FILE") };
}
