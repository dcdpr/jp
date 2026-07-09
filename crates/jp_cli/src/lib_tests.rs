use std::{env, fs, sync::Arc};

use camino::Utf8PathBuf;
use camino_tempfile::tempdir;
use clap::CommandFactory;
use jp_config::{
    AppConfig, PartialAppConfig,
    model::id::{PartialModelIdConfig, ProviderId},
    util::build,
};
use jp_conversation::{Conversation, ConversationId};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::{
    Workspace,
    session::{Session, SessionId, SessionSource},
    user_data_dir,
};
use relative_path::RelativePathBuf;
use serde_json::Value;
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

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs),
    )
    .unwrap()
}

fn config_with_model(provider: ProviderId, name: &str) -> AppConfig {
    let mut partial = AppConfig::new_test().to_partial();
    partial.assistant.model.id = PartialModelIdConfig {
        provider: Some(provider),
        name: Some(name.parse().unwrap()),
    }
    .into();

    build(partial).unwrap()
}

/// Helper: build a pipeline from a base partial + overrides, then return the
/// built partial (without conversation layer).
fn build_cfg(
    base: PartialAppConfig,
    overrides: &[KeyValueOrPath],
    workspace: Option<&Workspace>,
) -> Result<PartialAppConfig> {
    let pipeline = config_pipeline::ConfigPipeline::new(overrides, workspace, None, || Ok(base))?;
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

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_DIR", global_dir.as_str()) };

    write_config(
        &global_dir.join("config/.jp/config/skill/web.toml"),
        "assistant.name = 'from-global'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-global"));

    unsafe { std::env::remove_var("JP_GLOBAL_CONFIG_DIR") };
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_merges_global_and_workspace() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let ws_root = tmp.path().join("workspace");

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_DIR", global_dir.as_str()) };

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

    unsafe { std::env::remove_var("JP_GLOBAL_CONFIG_DIR") };
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_workspace_overrides_global() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let ws_root = tmp.path().join("workspace");

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_DIR", global_dir.as_str()) };

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

    unsafe { std::env::set_var("JP_GLOBAL_CONFIG_DIR", global_dir.as_str()) };

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

#[test]
#[serial(env_vars)]
fn query_model_override_persists_config_delta_through_run_inner() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");
    let global_dir = root.join("global");
    let user_data = root.join("user_data");
    let previous_cwd = env::current_dir().unwrap();
    let previous_jp_editor = env::var("JP_EDITOR").ok();
    let previous_visual = env::var("VISUAL").ok();
    let previous_editor = env::var("EDITOR").ok();

    unsafe { env::set_var("JP_GLOBAL_CONFIG_DIR", global_dir.as_str()) };
    unsafe { env::set_var("JP_USER_DATA_DIR", user_data.as_str()) };
    unsafe { env::remove_var("JP_EDITOR") };
    unsafe { env::remove_var("VISUAL") };
    unsafe { env::remove_var("EDITOR") };
    env::set_current_dir(root).unwrap();

    let fs_backend = Arc::new(FsStorageBackend::new(&storage).unwrap());
    let mut workspace = Workspace::new(root).with_backend(fs_backend.clone());
    let conversation_id = make_id(1000);
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "opus"));

    let lock = workspace
        .create_and_lock_conversation_with_id(
            conversation_id,
            Conversation::default(),
            base_config,
            None,
        )
        .unwrap();
    let conv = lock.into_mut();
    conv.update_metadata(|_| {});
    drop(conv);

    let cli = Cli::parse_from([
        "jp",
        "--workspace",
        root.as_str(),
        "query",
        "--id",
        &conversation_id.to_string(),
        "--model",
        "openai/gpt-4o",
    ]);

    let result = run_inner(cli, OutputFormat::TextPretty);
    assert!(
        matches!(result, Err(Error::Command(_))),
        "expected command error, got: {result:?}"
    );

    let raw = fs_backend
        .read_test_events_raw(&conversation_id)
        .expect("expected persisted events.json after failed query");
    let events: Value = serde_json::from_str(&raw).unwrap();
    let events = events.as_array().unwrap();

    let model_delta = events.iter().find(|event| {
        event.get("type").and_then(Value::as_str) == Some("config_delta")
            && event
                .get("delta")
                .and_then(|delta| delta.get("assistant"))
                .and_then(|assistant| assistant.get("model"))
                .is_some()
    });

    let model_delta = model_delta.expect("expected a persisted model config_delta event");
    assert_eq!(
        model_delta["delta"]["assistant"]["model"]["id"]["provider"],
        "openai"
    );
    assert_eq!(
        model_delta["delta"]["assistant"]["model"]["id"]["name"],
        "gpt-4o"
    );

    env::set_current_dir(previous_cwd).unwrap();
    unsafe { env::remove_var("JP_GLOBAL_CONFIG_DIR") };
    unsafe { env::remove_var("JP_USER_DATA_DIR") };

    match previous_jp_editor {
        Some(value) => unsafe { env::set_var("JP_EDITOR", value) },
        None => unsafe { env::remove_var("JP_EDITOR") },
    }
    match previous_visual {
        Some(value) => unsafe { env::set_var("VISUAL", value) },
        None => unsafe { env::remove_var("VISUAL") },
    }
    match previous_editor {
        Some(value) => unsafe { env::set_var("EDITOR", value) },
        None => unsafe { env::remove_var("EDITOR") },
    }
}

#[test]
#[serial(env_vars)]
fn query_model_override_persists_config_delta_through_session_targeting() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");
    let global_dir = root.join("global");
    let user_data = root.join("user_data");
    let previous_cwd = env::current_dir().unwrap();
    let previous_jp_session = env::var("JP_SESSION").ok();
    let previous_jp_editor = env::var("JP_EDITOR").ok();
    let previous_visual = env::var("VISUAL").ok();
    let previous_editor = env::var("EDITOR").ok();

    unsafe { env::set_var("JP_GLOBAL_CONFIG_DIR", global_dir.as_str()) };
    unsafe { env::set_var("JP_USER_DATA_DIR", user_data.as_str()) };
    unsafe { env::set_var("JP_SESSION", "jp-cli-test-session") };
    unsafe { env::remove_var("JP_EDITOR") };
    unsafe { env::remove_var("VISUAL") };
    unsafe { env::remove_var("EDITOR") };
    env::set_current_dir(root).unwrap();

    let mut workspace = Workspace::new(root);
    let user_root = user_data_dir().unwrap().join("workspace");
    let fs_backend = Arc::new(
        FsStorageBackend::new(&storage)
            .unwrap()
            .with_user_storage(&user_root, None, workspace.id().to_string())
            .unwrap(),
    );
    workspace = workspace.with_backend(fs_backend.clone());
    workspace.id().store(&storage).unwrap();

    let conversation_id = make_id(2000);
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "opus"));

    let lock = workspace
        .create_and_lock_conversation_with_id(
            conversation_id,
            Conversation::default(),
            base_config,
            None,
        )
        .unwrap();
    let conv = lock.into_mut();
    conv.update_metadata(|_| {});
    drop(conv);

    let session = Session {
        id: SessionId::new("jp-cli-test-session").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    workspace
        .record_session_activation(&session, conversation_id, chrono::Utc::now())
        .unwrap();

    let cli = Cli::parse_from([
        "jp",
        "--workspace",
        root.as_str(),
        "query",
        "--model",
        "openai/gpt-4o",
    ]);

    let result = run_inner(cli, OutputFormat::TextPretty);
    assert!(
        matches!(result, Err(Error::Command(_))),
        "expected command error, got: {result:?}"
    );

    let raw = fs_backend
        .read_test_events_raw(&conversation_id)
        .expect("expected persisted events.json after failed query");
    let events: Value = serde_json::from_str(&raw).unwrap();
    let events = events.as_array().unwrap();

    let model_delta = events.iter().find(|event| {
        event.get("type").and_then(Value::as_str) == Some("config_delta")
            && event
                .get("delta")
                .and_then(|delta| delta.get("assistant"))
                .and_then(|assistant| assistant.get("model"))
                .is_some()
    });

    let model_delta = model_delta.expect("expected a persisted model config_delta event");
    assert_eq!(
        model_delta["delta"]["assistant"]["model"]["id"]["provider"],
        "openai"
    );
    assert_eq!(
        model_delta["delta"]["assistant"]["model"]["id"]["name"],
        "gpt-4o"
    );

    env::set_current_dir(previous_cwd).unwrap();
    unsafe { env::remove_var("JP_GLOBAL_CONFIG_DIR") };
    unsafe { env::remove_var("JP_USER_DATA_DIR") };

    match previous_jp_session {
        Some(value) => unsafe { env::set_var("JP_SESSION", value) },
        None => unsafe { env::remove_var("JP_SESSION") },
    }
    match previous_jp_editor {
        Some(value) => unsafe { env::set_var("JP_EDITOR", value) },
        None => unsafe { env::remove_var("JP_EDITOR") },
    }
    match previous_visual {
        Some(value) => unsafe { env::set_var("VISUAL", value) },
        None => unsafe { env::remove_var("VISUAL") },
    }
    match previous_editor {
        Some(value) => unsafe { env::set_var("EDITOR", value) },
        None => unsafe { env::remove_var("EDITOR") },
    }
}

/// Verify that `resolve_config` consumes `default_id` so it doesn't leak into
/// the runtime `AppConfig`.
#[test]
fn resolve_config_consumes_default_id() {
    use jp_config::conversation::DefaultConversationId;

    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let mut workspace = Workspace::new(root);
    workspace.load_conversation_index();

    // Inject default_id into the base partial — no filesystem needed.
    let mut base = PartialAppConfig::new_test();
    base.conversation.default_id = Some(DefaultConversationId::LastActivated);

    let cli = Cli::try_parse_from(["jp", "conversation", "ls"]).unwrap();
    let (config, _handles, _start_new, _config_reset) = resolve_config(
        &cli.command,
        || Ok(base),
        &cli.globals.config,
        &mut workspace,
        None,
        None,
    )
    .unwrap();

    assert!(
        config.conversation.default_id.is_none(),
        "default_id should be consumed by resolve_config, got: {:?}",
        config.conversation.default_id,
    );
}

fn kv(s: &str) -> KeyValueOrPath {
    KeyValueOrPath::KeyValue(s.parse().unwrap())
}

/// `--no-cfg` expands to a leading `NONE` keyword for config resolution only;
/// the raw `--cfg` args stay as typed, so commands that re-consume them (e.g.
/// `config set`, which rejects reset keywords) don't see a synthetic keyword
/// ([RFD 038]).
#[test]
fn no_cfg_shorthand_does_not_leak_into_raw_cfg_args() {
    let cli = Cli::try_parse_from(["jp", "--no-cfg", "conversation", "ls"]).unwrap();

    let overrides = effective_cfg_overrides(&cli.globals);
    assert!(
        matches!(overrides.as_slice(), [KeyValueOrPath::Keyword(
            CfgKeyword::None
        )]),
        "expected a single synthetic NONE keyword, got: {overrides:?}",
    );

    // The raw args are untouched — `config set` and friends never see the
    // synthetic keyword.
    assert!(cli.globals.config.is_empty(), "{:?}", cli.globals.config);

    // Without `--no-cfg`, the list passes through unchanged.
    let cli = Cli::try_parse_from(["jp", "--cfg", "user.name=x", "conversation", "ls"]).unwrap();
    let overrides = effective_cfg_overrides(&cli.globals);
    assert_eq!(overrides.len(), 1);
    assert!(matches!(&overrides[0], KeyValueOrPath::KeyValue(_)));
}

/// A `--cfg` reset point must not resolve the targeted conversation's config:
/// the reset discards that layer, and resolving it can fail outright —
/// recovering a conversation with broken config is a reset use case ([RFD
/// 038]).
#[test]
fn resolve_config_reset_skips_broken_conversation_config() {
    use jp_conversation::stream::ResetDelta;

    let tmp = tempdir().unwrap();
    let mut workspace = Workspace::new(tmp.path());
    workspace.load_conversation_index();

    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(4000);
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );

    // Break the conversation's config resolution: a bare `Reset` with no
    // restoring `Apply` leaves the stream at program defaults, which lack
    // required fields, so `events.config()` fails.
    {
        let handle = workspace.acquire_conversation(&conversation_id).unwrap();
        let lock = workspace.test_lock(handle);
        lock.as_mut().update_events(|events| {
            events.add_config_delta(ResetDelta {
                timestamp: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
            });
        });
    }

    let id = conversation_id.to_string();
    let cli = Cli::try_parse_from(["jp", "query", "--id", &id, "hello"]).unwrap();

    // Without a reset point, the conversation layer is resolved and fails.
    let result = resolve_config(
        &cli.command,
        || Ok(base_config.to_partial()),
        &[],
        &mut workspace,
        None,
        None,
    );
    assert!(result.is_err(), "broken conversation config must propagate");

    // With `--cfg=NONE` (+ the required fields), the conversation layer is
    // skipped and resolution succeeds — the escape hatch works.
    let (config, _handles, _start_new, config_reset) = resolve_config(
        &cli.command,
        || Ok(base_config.to_partial()),
        &[
            KeyValueOrPath::Keyword(CfgKeyword::None),
            kv("assistant.model.id=openai/fresh-model"),
            kv("conversation.tools.*.run=ask"),
        ],
        &mut workspace,
        None,
        None,
    )
    .expect("--cfg=NONE must recover a broken conversation config");

    assert_eq!(
        config.assistant.model.id.resolved().name.as_ref(),
        "fresh-model"
    );
    assert!(config_reset.is_some());
}

/// Reset layers are persisted into conversation streams, so they must contain
/// resolved model IDs ([`PartialAppConfig::resolve_model_aliases`]): the
/// stream's own config resolution never resolves aliases ([RFD 038]).
#[test]
fn resolve_config_reset_workspace_layer_contains_resolved_model_ids() {
    use jp_config::model::id::PartialModelIdOrAliasConfig;

    let tmp = tempdir().unwrap();
    let mut workspace = Workspace::new(tmp.path());
    workspace.load_conversation_index();

    // The workspace config defines an alias and references it.
    let mut base = AppConfig::new_test().to_partial();
    base.providers.llm.aliases.insert(
        "fast".to_owned(),
        PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
            provider: Some(ProviderId::Openai),
            name: "gpt-4".parse().ok(),
        }),
    );
    base.assistant.model.id = PartialModelIdOrAliasConfig::Alias("fast".to_owned());

    let cli = Cli::try_parse_from(["jp", "conversation", "ls"]).unwrap();
    let (_config, _handles, _start_new, config_reset) = resolve_config(
        &cli.command,
        || Ok(base),
        &[KeyValueOrPath::Keyword(CfgKeyword::Workspace)],
        &mut workspace,
        None,
        None,
    )
    .unwrap();

    let reset_events = config_reset.expect("WORKSPACE keyword produces a reset");
    let ConfigReset::Workspace(layer) = &reset_events.reset else {
        panic!("expected a WORKSPACE reset, got: {:?}", reset_events.reset);
    };

    match &layer.assistant.model.id {
        PartialModelIdOrAliasConfig::Id(id) => {
            assert_eq!(id.provider, Some(ProviderId::Openai));
        }
        PartialModelIdOrAliasConfig::Alias(alias) => {
            panic!("alias `{alias}` persisted unresolved in the workspace layer");
        }
    }

    // The post layer is the diff between two resolved states; it must not
    // carry an alias either.
    assert!(
        !matches!(
            reset_events.post.assistant.model.id,
            PartialModelIdOrAliasConfig::Alias(_)
        ),
        "alias persisted unresolved in the post layer",
    );
}

/// Post-reset `--cfg` directives referencing an alias must persist the resolved
/// model ID, not the alias ([RFD 038]).
#[test]
fn resolve_config_reset_post_layer_contains_resolved_model_ids() {
    use jp_config::model::id::PartialModelIdOrAliasConfig;

    let tmp = tempdir().unwrap();
    let mut workspace = Workspace::new(tmp.path());
    workspace.load_conversation_index();

    let cli = Cli::try_parse_from(["jp", "conversation", "ls"]).unwrap();
    let (_config, _handles, _start_new, config_reset) = resolve_config(
        &cli.command,
        || unreachable!("NONE skips implicit loading"),
        &[
            KeyValueOrPath::Keyword(CfgKeyword::None),
            kv("providers.llm.aliases.fast=openai/gpt-4"),
            kv("assistant.model.id=fast"),
            kv("conversation.tools.*.run=ask"),
        ],
        &mut workspace,
        None,
        None,
    )
    .unwrap();

    let reset_events = config_reset.expect("NONE keyword produces a reset");
    match &reset_events.post.assistant.model.id {
        PartialModelIdOrAliasConfig::Id(id) => {
            assert_eq!(id.provider, Some(ProviderId::Openai));
            assert_eq!(id.name.as_ref().unwrap().to_string(), "gpt-4");
        }
        PartialModelIdOrAliasConfig::Alias(alias) => {
            panic!("alias `{alias}` persisted unresolved in the post layer");
        }
    }
}
