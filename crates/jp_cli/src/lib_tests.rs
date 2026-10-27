use std::{env, fmt::Write as _, fs, sync::Arc};

use camino::Utf8PathBuf;
use camino_tempfile::tempdir;
use clap::{Arg, Command, builder::ValueParser};
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
use crate::env_testing::EnvVarGuard;

#[test]
fn a_successful_run_keeps_its_trace_log_only_under_jp_debug() {
    assert!(should_persist_trace_log(RunOutcome::AsExpected, true));
    assert!(!should_persist_trace_log(RunOutcome::AsExpected, false));
}

#[test]
fn a_failed_run_always_keeps_its_trace_log() {
    // A command that exits non-zero to report a result (`grep` finding nothing)
    // is `AsExpected`, not `Failed`, and takes the rule above instead.
    assert!(should_persist_trace_log(RunOutcome::Failed, false));
    assert!(should_persist_trace_log(RunOutcome::Failed, true));
}

#[test]
fn a_run_just_under_five_minutes_does_not_repeat_the_trace_log_notice() {
    assert!(!should_repeat_trace_log_notice(
        Duration::from_mins(5) - Duration::from_nanos(1)
    ));
}

#[test]
fn a_five_minute_run_repeats_the_trace_log_notice() {
    assert!(should_repeat_trace_log_notice(Duration::from_mins(5)));
}

// The flag has to reach the printer to mean anything: it names a policy the
// printer enforces, and no call site consults it. A text format isolates the
// chrome half of `chrome_repaints`, which is otherwise also false under JSON.
#[test]
fn quiet_closes_the_printers_chrome_channel() {
    let quiet = Globals {
        quiet: true,
        ..Globals::default()
    };
    let printer = build_printer(&quiet, OutputFormat::TextPretty);
    assert!(!printer.chrome_repaints());
    printer.shutdown();

    let printer = build_printer(&Globals::default(), OutputFormat::TextPretty);
    assert!(printer.chrome_repaints());
    printer.shutdown();
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
    let pipeline = config_pipeline::ConfigPipeline::new(overrides, workspace, None, || Ok(base))?;
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
        // The same resolution `TitleGeneratorTask::new` performs, minus its
        // provider preflight, which would fail without credentials.
        model: jp_llm::title::resolve_model(&cfg, None),
        providers: cfg.providers.llm.clone(),
        events: jp_conversation::ConversationStream::new_test(),
        title: Some("generated".into()),
        max_response_bytes: cfg.assistant.request.max_response_bytes.bytes(),
        is_tty: false,
        notices: jp_llm::event::NoticeSink::new(|_| {}),
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
fn tracing_guard_exposes_temp_file_path_before_persisting() {
    let file = NamedUtf8TempFile::new().unwrap();
    let path = file.path().to_owned();
    let guard = TracingGuard {
        sink: Some(TraceSink::Temp(file)),
    };

    assert_eq!(guard.path(), Some(path.as_path()));
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

/// The whole command-line surface, as clap resolves it.
///
/// Pins every flag's spelling, arity, value type, relations, and help text, so
/// a change to how the commands are declared can only change what a user sees
/// on purpose.
#[test]
fn command_line_surface() {
    let mut cli = Cli::command().term_width(100).max_term_width(100);
    cli.build();

    let mut out = String::new();
    describe_command(&mut out, &cli, "jp", &[]);
    insta::assert_snapshot!(out);
}

/// Append `cmd`, its arguments, and then its subcommands to `out`.
///
/// `inherited` names the global arguments an ancestor already described, which
/// clap copies into every subcommand.
fn describe_command(out: &mut String, cmd: &Command, path: &str, inherited: &[String]) {
    let text = |s: Option<&clap::builder::StyledStr>| s.map(ToString::to_string);
    let hidden: Vec<_> = cmd
        .get_all_aliases()
        .filter(|a| !cmd.get_visible_aliases().any(|v| v == *a))
        .collect();

    writeln!(out, "## {path}").unwrap();
    writeln!(out, "about: {:?}", text(cmd.get_about())).unwrap();
    writeln!(out, "long_about: {:?}", text(cmd.get_long_about())).unwrap();
    writeln!(out, "after_help: {:?}", text(cmd.get_after_help())).unwrap();
    writeln!(
        out,
        "after_long_help: {:?}",
        text(cmd.get_after_long_help())
    )
    .unwrap();
    writeln!(
        out,
        "visible_aliases: {:?}",
        cmd.get_visible_aliases().collect::<Vec<_>>()
    )
    .unwrap();
    writeln!(out, "hidden_aliases: {hidden:?}").unwrap();
    writeln!(out, "hidden: {}", cmd.is_hide_set()).unwrap();
    writeln!(
        out,
        "external_subcommands: {:?}",
        cmd.is_allow_external_subcommands_set().then(|| cmd
            .get_external_subcommand_value_parser()
            .map(ValueParser::type_id))
    )
    .unwrap();

    // The version and author come from the package, so only their presence is
    // pinned: a release must not change the snapshot.
    let settings = [
        ("version", cmd.get_version().is_some()),
        ("long_version", cmd.get_long_version().is_some()),
        ("author", cmd.get_author().is_some()),
        ("subcommand_required", cmd.is_subcommand_required_set()),
        (
            "arg_required_else_help",
            cmd.is_arg_required_else_help_set(),
        ),
        (
            "disable_help_subcommand",
            cmd.is_disable_help_subcommand_set(),
        ),
        ("propagate_version", cmd.is_propagate_version_set()),
        (
            "subcommand_negates_reqs",
            cmd.is_subcommand_negates_reqs_set(),
        ),
        (
            "args_conflicts_with_subcommands",
            cmd.is_args_conflicts_with_subcommands_set(),
        ),
        ("flatten_help", cmd.is_flatten_help_set()),
    ];
    let set: Vec<_> = settings
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect();
    writeln!(out, "set: {set:?}").unwrap();
    writeln!(out, "usage: {}", cmd.clone().render_usage()).unwrap();
    for group in cmd.get_groups() {
        let args: Vec<_> = group.get_args().map(ToString::to_string).collect();
        writeln!(
            out,
            "group {}: {args:?} required={} multiple={}",
            group.get_id(),
            group.is_required_set(),
            group.clone().is_multiple()
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    let mut globals = inherited.to_vec();
    for arg in cmd.get_arguments() {
        let id = arg.get_id().to_string();
        if arg.is_global_set() {
            if inherited.contains(&id) {
                continue;
            }
            globals.push(id);
        }

        describe_arg(out, cmd, arg);
    }

    for sub in cmd.get_subcommands() {
        describe_command(out, sub, &format!("{path} {}", sub.get_name()), &globals);
    }
}

/// Append one argument of `cmd` to `out`.
fn describe_arg(out: &mut String, cmd: &Command, arg: &Arg) {
    let text = |s: Option<&clap::builder::StyledStr>| s.map(ToString::to_string);
    let aliases = arg.get_all_aliases().unwrap_or_default();
    let visible = arg.get_visible_aliases().unwrap_or_default();
    let hidden: Vec<_> = aliases.iter().filter(|a| !visible.contains(a)).collect();
    let possible: Vec<_> = arg
        .get_possible_values()
        .iter()
        .map(|v| v.get_name().to_owned())
        .collect();
    let conflicts: Vec<_> = cmd
        .get_arg_conflicts_with(arg)
        .iter()
        .map(|a| a.get_id().to_string())
        .collect();
    let flags = [
        ("required", arg.is_required_set()),
        ("global", arg.is_global_set()),
        ("hide", arg.is_hide_set()),
        ("last", arg.is_last_set()),
        ("require_equals", arg.is_require_equals_set()),
        ("trailing_var_arg", arg.is_trailing_var_arg_set()),
        ("allow_hyphen_values", arg.is_allow_hyphen_values_set()),
        ("exclusive", arg.is_exclusive_set()),
    ];
    let set: Vec<_> = flags
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect();

    writeln!(out, "### {}", arg.get_id()).unwrap();
    writeln!(out, "short: {:?}", arg.get_short()).unwrap();
    writeln!(out, "long: {:?}", arg.get_long()).unwrap();
    writeln!(out, "visible_aliases: {visible:?}").unwrap();
    writeln!(out, "hidden_aliases: {hidden:?}").unwrap();
    writeln!(out, "short_aliases: {:?}", arg.get_all_short_aliases()).unwrap();
    writeln!(out, "index: {:?}", arg.get_index()).unwrap();
    writeln!(out, "action: {:?}", arg.get_action()).unwrap();
    writeln!(out, "num_args: {:?}", arg.get_num_args()).unwrap();
    writeln!(out, "value_type: {:?}", arg.get_value_parser().type_id()).unwrap();
    writeln!(out, "possible_values: {possible:?}").unwrap();
    writeln!(out, "value_names: {:?}", arg.get_value_names()).unwrap();
    writeln!(out, "value_delimiter: {:?}", arg.get_value_delimiter()).unwrap();
    writeln!(out, "default_values: {:?}", arg.get_default_values()).unwrap();
    writeln!(out, "help_heading: {:?}", arg.get_help_heading()).unwrap();
    writeln!(out, "conflicts: {conflicts:?}").unwrap();
    writeln!(out, "set: {set:?}").unwrap();
    writeln!(out, "help: {:?}", text(arg.get_help())).unwrap();
    writeln!(out, "long_help: {:?}", text(arg.get_long_help())).unwrap();
    writeln!(out).unwrap();
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

    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", global_dir.as_str());

    write_config(
        &global_dir.join("config/.jp/config/skill/web.toml"),
        "assistant.name = 'from-global'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, None).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-global"));
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_merges_global_and_workspace() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let ws_root = tmp.path().join("workspace");

    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", global_dir.as_str());

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
        result.providers.llm.openrouter.api_key_env,
        Some("FROM_WS".into())
    );
}

#[test]
#[serial(env_vars)]
fn test_load_cli_cfg_args_workspace_overrides_global() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let ws_root = tmp.path().join("workspace");

    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", global_dir.as_str());

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

// A terminal is evidence of a user, but only until the user says otherwise.
#[test]
fn interactive_requires_a_terminal_and_no_opt_out() {
    assert!(interactive(false, true));
    assert!(!interactive(false, false));
    assert!(!interactive(true, true));
    assert!(!interactive(true, false));
}

/// `--no-interactive` is the flag; `--non-interactive` is the spelling most
/// other tools use, and both have to reach the same field.
#[test]
fn both_spellings_of_the_interactivity_flag_parse() {
    let cli = Cli::try_parse_from(["jp", "--no-interactive", "conversation", "ls"]).unwrap();
    assert!(cli.globals.no_interactive);

    let cli = Cli::try_parse_from(["jp", "--non-interactive", "conversation", "ls"]).unwrap();
    assert!(cli.globals.no_interactive, "the alias stopped working");
}

#[test]
#[serial(env_vars)]
fn non_interactive_env_var_accepts_1_and_true_only() {
    {
        let _guard = EnvVarGuard::set("JP_NONINTERACTIVE", "1");
        assert!(env_opt_out("JP_NONINTERACTIVE"));
    }
    {
        let _guard = EnvVarGuard::set("JP_NONINTERACTIVE", "true");
        assert!(env_opt_out("JP_NONINTERACTIVE"));
    }
    // `JP_NONINTERACTIVE=0` must not read as "set, therefore on" — a user who
    // exports it to turn the behaviour off would otherwise turn it on.
    {
        let _guard = EnvVarGuard::set("JP_NONINTERACTIVE", "0");
        assert!(!env_opt_out("JP_NONINTERACTIVE"));
    }
    {
        let _guard = EnvVarGuard::remove("JP_NONINTERACTIVE");
        assert!(!env_opt_out("JP_NONINTERACTIVE"));
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

    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", global_dir.as_str());

    let workspace = Workspace::in_memory(&ws_root);

    write_config(
        &global_dir.join("config/.jp/config/skill/web.toml"),
        "assistant.name = 'from-global'",
    );

    let partial = partial_with_load_paths(&[".jp/config"]);
    let overrides = vec![KeyValueOrPath::Path(Utf8PathBuf::from("skill/web"))];

    let result = build_cfg(partial, &overrides, Some(&workspace)).unwrap();
    assert_eq!(result.assistant.name.as_deref(), Some("from-global"));
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

    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", global_dir.as_str());
    let _user_data = EnvVarGuard::set("JP_USER_DATA_DIR", user_data.as_str());
    let _api_key = EnvVarGuard::set("JP_TEST_DUMMY_OPENAI_API_KEY", "dummy");
    let _jp_editor = EnvVarGuard::remove("JP_EDITOR");
    let _visual = EnvVarGuard::remove("VISUAL");
    let _editor = EnvVarGuard::remove("EDITOR");

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

    let cli = Cli::try_parse_from([
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
    ])
    .unwrap();

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

    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", global_dir.as_str());
    let _user_data = EnvVarGuard::set("JP_USER_DATA_DIR", user_data.as_str());
    let _session = EnvVarGuard::set("JP_SESSION", "jp-cli-test-session");
    let _api_key = EnvVarGuard::set("JP_TEST_DUMMY_OPENAI_API_KEY", "dummy");
    let _jp_editor = EnvVarGuard::remove("JP_EDITOR");
    let _visual = EnvVarGuard::remove("VISUAL");
    let _editor = EnvVarGuard::remove("EDITOR");

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

    let cli = Cli::try_parse_from([
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
    ])
    .unwrap();

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
}

// A prompt ended with Ctrl-C reports the shell's interrupt status, which a
// config error's message would lose.
#[test]
fn a_command_error_from_cli_flags_keeps_its_exit_status() {
    let error = cli_config_error(Box::new(cmd::Error::from(130_u8)));

    let Error::Command(error) = error else {
        panic!("expected the command error to pass through, got {error:?}");
    };
    assert_eq!(error.code.get(), 130);
}

#[test]
fn any_other_error_from_cli_flags_is_a_config_error() {
    let error = cli_config_error("`try` is not a configured tool".into());

    assert_matches::assert_matches!(
        error,
        Error::CliConfig(message) if message == "`try` is not a configured tool"
    );
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

    let mut cli = Cli::try_parse_from(["jp", "conversation", "ls"]).unwrap();
    let (config, _handles, _start_new, _config_reset) = resolve_config(
        &mut cli.command,
        || Ok(base),
        &cli.globals.config,
        &mut workspace,
        None,
        None,
        false,
    )
    .unwrap();

    assert!(
        config.conversation.default_id.is_none(),
        "default_id should be consumed by resolve_config, got: {:?}",
        config.conversation.default_id,
    );
}

/// `jp conversation compact --model` has to travel `Commands` -> `Conversation`
/// -> `Compact` to reach the config.
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
    let mut cli = Cli::try_parse_from([
        "jp",
        "conversation",
        "compact",
        &conversation_id.to_string(),
        "--model",
        "gpt",
    ])
    .unwrap();
    let (config, _handles, _start_new, _config_reset) = resolve_config(
        &mut cli.command,
        || Ok(base),
        &cli.globals.config,
        &mut workspace,
        None,
        None,
        false,
    )
    .unwrap();

    assert_eq!(
        config.assistant.model.id.resolved().to_string(),
        "openai/gpt-5"
    );
}

#[test]
fn resolve_config_rejects_a_cleared_model_without_changing_the_conversation() {
    let tmp = tempdir().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    workspace.load_conversation_index();
    let base = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(4002);
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base),
    );
    let id = conversation_id.to_string();
    let mut cli = Cli::try_parse_from([
        "jp",
        "query",
        "--id",
        &id,
        "--cfg",
        "assistant.model.id:=null",
        "hello",
    ])
    .unwrap();

    let error = resolve_config(
        &mut cli.command,
        || Ok(base.to_partial()),
        &cli.globals.config,
        &mut workspace,
        None,
        None,
        false,
    )
    .unwrap_err();

    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let events = workspace.events(&handle).unwrap();
    assert_eq!(events.config().unwrap(), *base);
    assert_eq!(events.config_deltas().count(), 0);
    let (code, rendered) = parse_error(cmd::Error::from(error), OutputFormat::TextPretty);
    assert_eq!(code, 1);
    assert_eq!(
        rendered,
        "Config error\n\n    Missing required value for field assistant.model.id.provider."
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
    let mut workspace = Workspace::in_memory(tmp.path());
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
    let mut cli = Cli::try_parse_from(["jp", "query", "--id", &id, "hello"]).unwrap();

    // Without a reset point, the conversation layer is resolved and fails.
    let result = resolve_config(
        &mut cli.command,
        || Ok(base_config.to_partial()),
        &[],
        &mut workspace,
        None,
        None,
        false,
    );
    assert!(result.is_err(), "broken conversation config must propagate");

    // With `--cfg=NONE` (+ the required fields), the conversation layer is
    // skipped and resolution succeeds — the escape hatch works.
    let (config, _handles, _start_new, config_reset) = resolve_config(
        &mut cli.command,
        || Ok(base_config.to_partial()),
        &[
            KeyValueOrPath::Keyword(CfgKeyword::None),
            kv("assistant.model.id=openai/fresh-model"),
            kv("conversation.tools.*.run=ask"),
        ],
        &mut workspace,
        None,
        None,
        false,
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
    let mut workspace = Workspace::in_memory(tmp.path());
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

    let mut cli = Cli::try_parse_from(["jp", "conversation", "ls"]).unwrap();
    let (_config, _handles, _start_new, config_reset) = resolve_config(
        &mut cli.command,
        || Ok(base),
        &[KeyValueOrPath::Keyword(CfgKeyword::Workspace)],
        &mut workspace,
        None,
        None,
        false,
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
    let mut workspace = Workspace::in_memory(tmp.path());
    workspace.load_conversation_index();

    let mut cli = Cli::try_parse_from(["jp", "conversation", "ls"]).unwrap();
    let (_config, _handles, _start_new, config_reset) = resolve_config(
        &mut cli.command,
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
        false,
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

// Workspace-root selection by ID moved into the bootstrap step (RFD 087
// phase 2/3); its behavior is covered by `bootstrap_tests` and
// `cmd::workspace::target` tests.
