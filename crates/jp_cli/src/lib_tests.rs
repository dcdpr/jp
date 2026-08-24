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

#[test]
fn a_piped_successful_run_never_announces_its_trace_log() {
    // Two uninvited lines on stderr corrupt any program that owns the screen,
    // and in `jp … | fzf` stderr *is* the terminal. `JP_DEBUG` is pinned on
    // here because it's the only reason a non-failing run would print at all.
    assert!(!should_report_trace_log(
        RunOutcome::AsExpected,
        false,
        true
    ));
}

#[test]
fn a_successful_run_on_a_terminal_announces_its_trace_log_only_under_jp_debug() {
    assert!(should_report_trace_log(RunOutcome::AsExpected, true, true));
    assert!(!should_report_trace_log(
        RunOutcome::AsExpected,
        true,
        false
    ));
}

#[test]
fn a_failed_run_announces_its_trace_log_even_when_piped() {
    // Diagnosing a failure beats keeping the pipeline clean, so neither the tty
    // nor `JP_DEBUG` has a say. A command that exits non-zero to report a
    // result (`grep` finding nothing) is `AsExpected`, not `Failed`, and takes
    // the rules above instead.
    assert!(should_report_trace_log(RunOutcome::Failed, false, false));
    assert!(should_report_trace_log(RunOutcome::Failed, true, false));
}

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
    let pipeline = config_pipeline::ConfigPipeline::new(base, overrides, workspace, None)?;
    pipeline.partial_without_conversation()
}

/// Persistence backend whose every write fails with a full disk.
#[derive(Debug)]
struct AlwaysFullBackend;

impl jp_storage::backend::PersistBackend for AlwaysFullBackend {
    fn write(
        &self,
        _id: &ConversationId,
        _metadata: &Conversation,
        _events: &jp_conversation::ConversationStream,
        _projection: jp_storage::backend::Projection,
    ) -> std::result::Result<(), jp_storage::Error> {
        Err(jp_storage::Error::write_failed(
            camino::Utf8Path::new("/data/conv/events.json"),
            std::io::Error::from(std::io::ErrorKind::StorageFull),
        ))
    }

    fn remove(&self, _id: &ConversationId) -> std::result::Result<(), jp_storage::Error> {
        Ok(())
    }

    fn archive(&self, _id: &ConversationId) -> std::result::Result<(), jp_storage::Error> {
        Ok(())
    }

    fn unarchive(&self, _id: &ConversationId) -> std::result::Result<(), jp_storage::Error> {
        Ok(())
    }
}

#[test]
fn a_cancelled_command_future_still_reports_its_persist_failure() {
    // Mirrors `run_inner`'s shutdown arm: on Ctrl-C the command future is
    // dropped mid-flight, so the drain inside it never runs. The command arm
    // here parks forever after dirtying the conversation, which holds the
    // window open — cancellation always lands while the scope is still alive,
    // rather than racing a sleep.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let mut workspace = Workspace::in_memory("root").with_persist(Arc::new(AlwaysFullBackend));
    let lock = workspace
        .create_and_lock_conversation(
            Conversation::default(),
            Arc::new(AppConfig::new_test()),
            None,
        )
        .unwrap();

    let output: cmd::Output = rt.block_on(async {
        tokio::select! {
            biased;
            () = async {
                let conv = lock.as_mut();
                conv.update_metadata(|m| m.title = Some("unsaved".into()));
                std::future::pending::<()>().await;
            } => unreachable!("the command arm never completes"),
            () = std::future::ready(()) => Err(cmd::Error::interrupted()),
        }
    });
    drop(lock);

    let error = cmd::fold_persist_failure(output, workspace.take_persist_failure())
        .expect_err("the run was interrupted");

    // The interrupt stays the headline and keeps its exit code; the fact that
    // nothing was saved rides along instead of being lost to the log file.
    assert_eq!(error.message.as_deref(), Some("Interrupted"));
    assert_eq!(error.code.get(), 130);
    assert_eq!(
        error
            .metadata
            .iter()
            .find(|(key, _)| key == "persist_failure")
            .map(|(_, value)| value.as_str().unwrap_or_default()),
        Some("Storage error: no space left on device while writing /data/conv/events.json")
    );
}

#[test]
fn a_background_task_persist_failure_is_recorded_after_the_command_finished() {
    // `TitleGeneratorTask::sync` takes a conversation lock of its own and
    // swallows a failed `flush()` into a `warn!`. The still-dirty scope's drop
    // then records on the workspace, which happens while background tasks are
    // draining — after the command's own drain has already run. That is why
    // `run_inner` folds a second time once the drain returns.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let mut workspace = Workspace::in_memory("root").with_persist(Arc::new(AlwaysFullBackend));

    let lock = workspace
        .create_and_lock_conversation(
            Conversation::default(),
            Arc::new(AppConfig::new_test()),
            None,
        )
        .unwrap();
    let id = lock.id();
    // `sync` acquires the lock itself and skips when it is already held, which
    // would make every assertion below pass vacuously.
    drop(lock);
    assert!(
        workspace.take_persist_failure().is_none(),
        "setup must record nothing, or the failure asserted below proves nothing"
    );

    let cfg = AppConfig::new_test();
    let task = Box::new(jp_task::task::TitleGeneratorTask {
        conversation_id: id,
        model_id: cfg.assistant.model.id.resolved().clone(),
        providers: cfg.providers.llm.clone(),
        events: jp_conversation::ConversationStream::new_test(),
        title: Some("generated".into()),
        max_response_bytes: cfg.assistant.request.max_response_bytes,
        is_tty: false,
    });

    // `sync` reports success: it only logs the write failure.
    rt.block_on(jp_task::Task::sync(task, &mut workspace))
        .expect("sync swallows the write failure");

    let recorded = workspace
        .take_persist_failure()
        .expect("the title task's failed write is recorded on the workspace");
    assert_eq!(
        recorded.to_string(),
        "Storage error: no space left on device while writing /data/conv/events.json"
    );

    // What the post-drain fold makes of it: a run that would otherwise have
    // exited zero now carries the failure.
    let error = cmd::fold_persist_failure(Ok(()), Some(recorded))
        .expect_err("an unsaved conversation must not exit zero");
    assert_eq!(error.message.as_deref(), Some("No space left on device"));
}

#[test]
fn tracing_guard_persist_returns_explicit_log_file_path() {
    // `--log-file <path>`: the file lives wherever the caller put it; persist
    // just hands the path back.
    let guard = TracingGuard {
        sink: Some(TraceSink::Path(Utf8PathBuf::from("/tmp/x.jsonl"))),
    };
    assert_eq!(guard.persist(), Some(Utf8PathBuf::from("/tmp/x.jsonl")));
}

#[test]
fn tracing_guard_persist_keeps_temp_file_on_disk() {
    // Without `--log-file`, persist disarms the temp file's delete-on-drop
    // and returns its path.
    let file = NamedUtf8TempFile::new().unwrap();
    let guard = TracingGuard {
        sink: Some(TraceSink::Temp(file)),
    };

    let path = guard.persist().unwrap();
    assert!(path.exists());
    fs::remove_file(path).unwrap();
}

#[test]
fn test_cli() {
    Cli::command().debug_assert();
}

#[test]
fn test_load_cli_cfg_args_workspace_root() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let workspace = Workspace::in_memory(root);

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

    let workspace = Workspace::in_memory(&ws_root);

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

    let workspace = Workspace::in_memory(&ws_root);

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
    let workspace = Workspace::in_memory(root);

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
    let workspace = Workspace::in_memory(root);

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

    let workspace = Workspace::in_memory(&ws_root);

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
    unsafe { env::set_var("JP_TEST_DUMMY_OPENAI_API_KEY", "dummy") };
    unsafe { env::remove_var("JP_EDITOR") };
    unsafe { env::remove_var("VISUAL") };
    unsafe { env::remove_var("EDITOR") };
    env::set_current_dir(root).unwrap();

    let fs_backend = Arc::new(FsStorageBackend::new(&storage).unwrap());
    let mut workspace = Workspace::in_memory(root).with_backend(fs_backend.clone());
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
        // An inline query so the request builds without an editor and the
        // failure happens at the LLM stage: the CLI config delta is only
        // recorded once a non-empty request exists, so a query aborted
        // before that point (e.g. by the credential preflight) deliberately
        // leaves no config event behind.
        "hello",
        // Pass the credential preflight with a dummy key, then fail at the
        // request stage against an unroutable loopback address so the test
        // never touches the network, even on machines where a real
        // `OPENAI_API_KEY` is set.
        "--cfg",
        "providers.llm.openai.api_key_env=JP_TEST_DUMMY_OPENAI_API_KEY",
        "--cfg",
        "providers.llm.openai.base_url=http://127.0.0.1:9",
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
    unsafe { env::remove_var("JP_TEST_DUMMY_OPENAI_API_KEY") };

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
    unsafe { env::set_var("JP_TEST_DUMMY_OPENAI_API_KEY", "dummy") };
    unsafe { env::remove_var("JP_EDITOR") };
    unsafe { env::remove_var("VISUAL") };
    unsafe { env::remove_var("EDITOR") };
    env::set_current_dir(root).unwrap();

    let mut workspace = Workspace::in_memory(root);
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
        // See the run_inner variant above: an inline query moves the failure
        // past the turn-start commit point so the delta is persisted, and
        // the dummy key plus unroutable base URL make the LLM stage fail
        // without touching the network.
        "hello",
        "--cfg",
        "providers.llm.openai.api_key_env=JP_TEST_DUMMY_OPENAI_API_KEY",
        "--cfg",
        "providers.llm.openai.base_url=http://127.0.0.1:9",
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
    unsafe { env::remove_var("JP_TEST_DUMMY_OPENAI_API_KEY") };

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

    let mut workspace = Workspace::in_memory(root);
    workspace.load_conversation_index();

    // Inject default_id into the base partial — no filesystem needed.
    let mut base = PartialAppConfig::new_test();
    base.conversation.default_id = Some(DefaultConversationId::LastActivated);

    let cli = Cli::try_parse_from(["jp", "conversation", "ls"]).unwrap();
    let (config, _handles, _start_new) = resolve_config(
        &cli.command,
        base,
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

/// `jp conversation compact --model` has to travel `Commands` -\>
/// `Conversation` -\> `Compact` to reach the config.
/// A missing delegation arm anywhere on that chain makes the flag a silent
/// no-op, which no test on `Compact` alone can catch.
#[test]
fn resolve_config_applies_the_compact_model_flag() {
    use jp_config::model::id::PartialModelIdOrAliasConfig;

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let fs_backend = Arc::new(FsStorageBackend::new(&storage).unwrap());
    let mut workspace = Workspace::in_memory(root).with_backend(fs_backend);
    let conversation_id = make_id(3000);
    workspace
        .create_and_lock_conversation_with_id(
            conversation_id,
            Conversation::default(),
            Arc::new(config_with_model(ProviderId::Anthropic, "opus")),
            None,
        )
        .unwrap();

    let mut base = PartialAppConfig::new_test();
    base.providers.llm.aliases.insert(
        "gpt".to_owned(),
        PartialModelIdOrAliasConfig::from("openai/gpt-5"),
    );

    // `compact` takes the conversation as a positional argument.
    let cli = Cli::try_parse_from([
        "jp",
        "conversation",
        "compact",
        &conversation_id.to_string(),
        "--model",
        "gpt",
    ])
    .unwrap();
    let (config, _handles, _start_new) = resolve_config(
        &cli.command,
        base,
        &cli.globals.config,
        &mut workspace,
        None,
        None,
    )
    .unwrap();

    assert_eq!(
        config.assistant.model.id.resolved().to_string(),
        "openai/gpt-5"
    );
}

// Workspace-root selection by ID moved into the bootstrap step (RFD 087
// phase 2/3); its behavior is covered by `bootstrap_tests` and
// `cmd::workspace::target` tests.
