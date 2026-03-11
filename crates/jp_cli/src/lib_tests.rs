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

    let result = load_cli_cfg_args(partial, &overrides, Some(&workspace)).unwrap();
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

    let result = load_cli_cfg_args(partial, &overrides, None).unwrap();
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

    // Global sets name, workspace sets system_prompt (different fields)
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

    let result = load_cli_cfg_args(partial, &overrides, Some(&workspace)).unwrap();

    // Both fields should be present: global loaded first, workspace merged on top
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

    // Both set the same field; workspace (higher precedence) should win
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

    let result = load_cli_cfg_args(partial, &overrides, Some(&workspace)).unwrap();
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

    let err = load_cli_cfg_args(partial, &overrides, Some(&workspace)).unwrap_err();
    match err {
        Error::MissingConfigFile { path, searched } => {
            assert_eq!(path.as_str(), "skill/missing");
            assert!(
                searched.iter().any(|p| p.as_str().contains(".jp/config")),
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

    let result = load_cli_cfg_args(partial, &overrides, Some(&workspace)).unwrap();
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

    let result = load_cli_cfg_args(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("direct"));
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_no_roots_errors() {
    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("foobar/baz"))];

    let err = load_cli_cfg_args(partial, &overrides, None).unwrap_err();
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

    let result = load_cli_cfg_args(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("test"));
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_global_only_when_workspace_has_no_match() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let ws_root = tmp.path().join("workspace");

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_FILE", global_dir.as_str()) };

    let workspace = Workspace::new(&ws_root);

    // Only global has the file, workspace does not
    write_config(
        &global_dir.join("config/.jp/config/skill/web.toml"),
        "assistant.name = 'from-global'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = load_cli_cfg_args(partial, &overrides, Some(&workspace)).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-global"));

    unsafe { std::env::remove_var("JP_GLOBAL_CONFIG_FILE") };
}
