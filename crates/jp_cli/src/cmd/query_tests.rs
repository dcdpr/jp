use std::sync::Arc;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use jp_config::{
    AppConfig, PartialAppConfig, ToPartial,
    conversation::tool::{Enable, PartialToolConfig},
    model::id::{ModelIdConfig, PartialModelIdConfig, ProviderId},
    util::build,
};
use jp_conversation::{
    Conversation, ConversationId, ConversationStream,
    event::{ChatRequest, ChatResponse},
};
use jp_inquire::prompt::MockPromptBackend;
use jp_llm::{
    Provider,
    provider::mock::MockProvider,
    tool::{InvocationContext, builtin::BuiltinExecutors, executor::ExecutorSource},
};
use jp_printer::{OutputFormat, Printer};
use jp_workspace::{ConversationHandle, Workspace};
use relative_path::RelativePathBuf;
use serde_json::Value;
use tokio::sync::broadcast;

use super::*;
use crate::{KeyValueOrPath, config_pipeline::ConfigPipeline};

fn make_partial_with_tools() -> PartialAppConfig {
    let mut partial = PartialAppConfig::default();
    partial.conversation.tools.tools = IndexMap::from_iter([
        ("implicitly_enabled_tool".into(), PartialToolConfig {
            enable: None,
            ..Default::default()
        }),
        ("explicitly_enabled_tool".into(), PartialToolConfig {
            enable: Some(Enable::On),
            ..Default::default()
        }),
        ("explicitly_disabled_tool".into(), PartialToolConfig {
            enable: Some(Enable::Off),
            ..Default::default()
        }),
        ("explicit_tool".into(), PartialToolConfig {
            enable: Some(Enable::Explicit),
            ..Default::default()
        }),
    ]);
    partial
}

/// Helper to build directives from a list.
fn directives(ds: Vec<ToolDirective>) -> ToolDirectives {
    ToolDirectives(ds)
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
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

fn empty_executor_source() -> Box<dyn ExecutorSource> {
    Box::new(tool::executor::TerminalExecutorSource::new(
        BuiltinExecutors::new(),
        &[],
        std::sync::Arc::new(crate::access::approvals::ApprovalStore::default()),
        InvocationContext::default(),
    ))
}

fn build_query_config(
    workspace: &Workspace,
    base: PartialAppConfig,
    cfg_args: &[KeyValueOrPath],
    query: &Query,
    handle: Option<&ConversationHandle>,
) -> AppConfig {
    let pipeline = ConfigPipeline::new(base, cfg_args, Some(workspace), None).unwrap();

    let conversation_partial = handle.map(|handle| {
        query
            .apply_conversation_config(workspace, PartialAppConfig::default(), None, handle)
            .unwrap()
    });

    let mut partial = match conversation_partial {
        Some(conversation_partial) => pipeline.partial_with_conversation(conversation_partial),
        None => pipeline.partial_without_conversation(),
    }
    .unwrap();

    partial = query
        .apply_cli_config(Some(workspace), partial, None)
        .unwrap();

    build(partial).unwrap()
}

async fn run_mock_turn(
    root: &camino::Utf8Path,
    cfg: &AppConfig,
    lock: &jp_workspace::ConversationLock,
    prompt: &str,
    response: &str,
) {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message(response));
    let model = provider
        .model_details(&cfg.assistant.model.id.resolved().name)
        .await
        .unwrap();
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (_signal_tx, signal_rx) = broadcast::channel(16);

    turn_loop::run_turn_loop(
        Arc::clone(&provider),
        &model,
        cfg,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        lock,
        jp_config::assistant::tool_choice::ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        tool::ToolCoordinator::new(cfg.conversation.tools.clone(), empty_executor_source()),
        ChatRequest::from(prompt),
        InvocationContext::default(),
    )
    .await
    .unwrap();
}

#[test]
#[expect(clippy::too_many_lines)]
fn test_query_tools_and_no_tools() {
    // Create a partial configuration with a few tools.
    let mut partial = make_partial_with_tools();

    // Keep all tools as-is (no directives).
    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        None,
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::On)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::Off)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Explicit)
    );

    // Disable one tool.
    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::Disable(
                "implicitly_enabled_tool".into(),
            )]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::Off),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::On)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::Off)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Explicit)
    );

    // Enable one tool.
    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::Enable(
                "explicitly_disabled_tool".into(),
            )]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::Off),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::On)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::On)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Explicit)
    );

    // Enable all tools -- explicit tools should stay explicit.
    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::EnableAll]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::On),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::On)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::On)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Explicit),
        "explicit tools should NOT be enabled by --tools without arguments"
    );

    // Disable all tools.
    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::DisableAll]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::Off),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::Off)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::Off)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Off)
    );

    // Enable multiple tools.
    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![
                ToolDirective::Enable("explicitly_disabled_tool".into()),
                ToolDirective::Enable("explicitly_enabled_tool".into()),
            ]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::Off),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::On)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::On)
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Off)
    );
}

#[test]
fn test_explicit_tool_enabled_by_name() {
    // An explicit tool can be activated by naming it with --tools.
    let mut partial = make_partial_with_tools();

    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::Enable("explicit_tool".into())]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::On),
        "explicit tools should be enabled when named specifically"
    );
}

#[test]
fn test_enable_all_and_explicit_by_name() {
    // `-t -t explicit_tool` should enable all non-explicit tools AND
    // enable the named explicit tool.
    let mut partial = make_partial_with_tools();

    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![
                ToolDirective::EnableAll,
                ToolDirective::Enable("explicit_tool".into()),
            ]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::On),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::On),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::On),
        "naming an explicit tool alongside --tools should enable it"
    );
}

#[test]
fn test_enable_all_skips_unnamed_explicit() {
    // Bare `-t` should enable everything except explicit tools.
    let mut partial = make_partial_with_tools();

    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::EnableAll]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::On),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Explicit),
        "bare --tools should not enable explicit tools"
    );
}

// --- New tests for ordered/interleaved evaluation (RFD 008) ---

#[test]
fn test_interleaved_disable_all_then_enable_named() {
    // `--no-tools --tool=explicitly_disabled_tool`
    // Should disable everything first, then re-enable only the named tool.
    let mut partial = make_partial_with_tools();

    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![
                ToolDirective::DisableAll,
                ToolDirective::Enable("explicitly_disabled_tool".into()),
            ]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::Off),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::Off),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::On),
        "named tool should be re-enabled after disable-all"
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Off),
    );
}

#[test]
fn test_interleaved_enable_all_then_disable_named() {
    // `--tool --no-tools=implicitly_enabled_tool`
    // Should enable everything, then carve out one exception.
    let mut partial = make_partial_with_tools();

    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![
                ToolDirective::EnableAll,
                ToolDirective::Disable("implicitly_enabled_tool".into()),
            ]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::Off),
        "the carved-out tool should be disabled"
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::On),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::On),
    );
}

#[test]
fn test_interleaved_disable_all_then_enable_all() {
    // `--no-tools --tool` is now well-defined: disable all, then enable all.
    // DisableAll sets explicit_tool to Off, then EnableAll sees Off (not
    // Explicit) and enables it. Sequential evaluation doesn't preserve the
    // original Explicit marker once it's been overwritten.
    let mut partial = make_partial_with_tools();

    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::DisableAll, ToolDirective::EnableAll]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::On),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::On),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::On),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::On),
        "DisableAll wiped Explicit to Off, so EnableAll sees Off and enables it"
    );
}

#[test]
fn test_interleaved_three_step_composition() {
    // `--no-tools --tool=explicitly_disabled_tool --no-tools=explicitly_disabled_tool`
    // Disable all, enable one, then disable that same one again.
    let mut partial = make_partial_with_tools();

    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![
                ToolDirective::DisableAll,
                ToolDirective::Enable("explicitly_disabled_tool".into()),
                ToolDirective::Disable("explicitly_disabled_tool".into()),
            ]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    // Everything should be off -- the final disable reverts the enable.
    assert_eq!(
        partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
        Some(Enable::Off),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
        Some(Enable::Off),
    );
    assert_eq!(
        partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
        Some(Enable::Off),
        "final disable should override the intermediate enable"
    );
    assert_eq!(
        partial.conversation.tools.tools["explicit_tool"].enable,
        Some(Enable::Off),
    );
}

#[test]
fn query_model_override_is_persisted_as_config_delta() {
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(1000);

    let mut workspace = Workspace::new("/tmp/test");
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );

    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    let query = Query {
        model: Some("openai/gpt-4o".to_owned()),
        ..Default::default()
    };

    let partial = query
        .apply_cli_config(None, base_config.to_partial(), None)
        .unwrap();
    let runtime_config = build(partial).unwrap();

    let delta = get_config_delta_from_cli(&runtime_config, &lock)
        .unwrap()
        .expect("expected query model override to produce a config delta");

    lock.as_mut()
        .update_events(|events| events.add_config_delta(delta));

    let events = lock.events().clone();
    let merged = events.config().unwrap();
    let model_id = merged.assistant.model.id.resolved();

    assert_eq!(model_id.provider, ProviderId::Openai);
    assert_eq!(model_id.name.as_ref(), "gpt-4o");

    let (_base, serialized_events) = events.to_parts().unwrap();
    assert!(
        serialized_events
            .iter()
            .any(|event| { event.get("type").and_then(Value::as_str) == Some("config_delta") }),
        "expected events.json to contain a config_delta event",
    );
}

#[test]
fn query_cfg_sourced_compaction_persists_as_config_delta() {
    // Compaction config that arrives through the config layers (e.g. `-c
    // compaction/heavy` or `--cfg conversation.compaction.rules=...`) is
    // ordinary conversation config: it must persist as a delta like any other
    // key. The one-shot inline DSL is the only compaction input kept out of the
    // config (see `inline_compact_dsl_is_not_written_into_query_config`).
    use jp_config::{
        conversation::compaction::{PartialCompactionRuleConfig, ReasoningMode},
        types::vec::MergeableVec,
    };

    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(2000);

    let mut workspace = Workspace::new("/tmp/test");
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );
    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    // Stand in for a `-c`/`--cfg` layer that sets a compaction rule differing
    // from the stored conversation config (reasoning-only vs the built-in
    // reasoning + tools default).
    let mut partial = base_config.to_partial();
    partial.conversation.compaction.rules = MergeableVec::Vec(vec![PartialCompactionRuleConfig {
        reasoning: Some(ReasoningMode::Strip),
        ..Default::default()
    }]);
    let runtime_config = build(partial).unwrap();

    let delta = get_config_delta_from_cli(&runtime_config, &lock)
        .unwrap()
        .expect("cfg-sourced compaction config should produce a delta");

    assert!(
        !delta.conversation.compaction.rules.is_empty(),
        "compaction config from the config layers must persist as a conversation delta",
    );
}

#[test]
fn inline_compact_dsl_is_not_written_into_query_config() {
    // The inline `-k SPEC` plan is applied as overlay events at query time, not
    // as config. `apply_cli_config` must leave `conversation.compaction`
    // untouched so the spec is never recorded as a conversation config delta and
    // replayed by a future bare `--compact`.
    use crate::cmd::compact_flag::CompactFlag;

    // Start from an empty partial so any compaction rules in the result could
    // only have been written by `apply_cli_config` itself.
    let base = jp_config::PartialAppConfig::default();
    let query = Query {
        compact: CompactFlag {
            use_config_rules: false,
            specs: vec!["s:..-3".parse().unwrap()],
        },
        ..Default::default()
    };

    let partial = query.apply_cli_config(None, base, None).unwrap();
    assert!(
        partial.conversation.compaction.rules.is_empty(),
        "inline -k DSL must not be written into the config partial",
    );
}

#[tokio::test]
async fn query_sequence_new_cfg_profile_then_model_override_persists_for_plain_query() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".jp/config")).unwrap();
    std::fs::write(
        root.join(".jp/config/dev.toml"),
        "assistant.model.id = 'anthropic/dev-model'\n",
    )
    .unwrap();

    let mut base = AppConfig::new_test().to_partial();
    base.config_load_paths = Some(vec![RelativePathBuf::from(".jp/config")]);
    base.providers.llm.aliases.insert(
        "gpt".to_owned(),
        ModelIdConfig {
            provider: ProviderId::Openai,
            name: "gpt-model".parse().unwrap(),
        }
        .to_partial()
        .into(),
    );

    let mut workspace = Workspace::new(root);

    let query1 = Query {
        new_conversation: true,
        query: Some(vec!["is this thing on?".to_owned()]),
        ..Default::default()
    };
    let cfg1 = build_query_config(
        &workspace,
        base.clone(),
        &[KeyValueOrPath::Path("dev".into())],
        &query1,
        None,
    );
    let model1 = cfg1.assistant.model.id.resolved();
    assert_eq!(model1.provider, ProviderId::Anthropic);
    assert_eq!(model1.name.as_ref(), "dev-model");

    let lock1 = workspace
        .create_and_lock_conversation(Conversation::default(), Arc::new(cfg1.clone()), None)
        .unwrap();
    let conversation_id = lock1.id();
    run_mock_turn(
        root,
        &cfg1,
        &lock1,
        "is this thing on?",
        "Yes, loud and clear.",
    )
    .await;
    drop(lock1);

    let handle2 = workspace.acquire_conversation(&conversation_id).unwrap();
    let query2 = Query {
        model: Some("gpt".to_owned()),
        query: Some(vec!["are you there?".to_owned()]),
        ..Default::default()
    };
    let cfg2 = build_query_config(&workspace, base.clone(), &[], &query2, Some(&handle2));
    let lock2 = workspace.test_lock(handle2);
    let delta = get_config_delta_from_cli(&cfg2, &lock2)
        .unwrap()
        .expect("expected model override to persist");
    lock2
        .as_mut()
        .update_events(|events| events.add_config_delta(delta));
    run_mock_turn(root, &cfg2, &lock2, "are you there?", "Yes.").await;
    drop(lock2);

    let handle3 = workspace.acquire_conversation(&conversation_id).unwrap();
    let query3 = Query {
        query: Some(vec!["plain query".to_owned()]),
        ..Default::default()
    };
    let cfg3 = build_query_config(&workspace, base, &[], &query3, Some(&handle3));
    let model3 = cfg3.assistant.model.id.resolved();
    assert_eq!(model3.provider, ProviderId::Openai);
    assert_eq!(model3.name.as_ref(), "gpt-model");

    let events = workspace.events(&handle3).unwrap();
    let serialized = events.to_parts().unwrap().1;
    let model_delta = serialized.iter().find(|event| {
        event.get("type").and_then(Value::as_str) == Some("config_delta")
            && event
                .get("delta")
                .and_then(|delta| delta.get("assistant"))
                .and_then(|assistant| assistant.get("model"))
                .is_some()
    });
    let model_delta = model_delta.expect("expected a model config_delta event");
    assert_eq!(
        model_delta["delta"]["assistant"]["model"]["id"]["provider"],
        "openai"
    );
    assert_eq!(
        model_delta["delta"]["assistant"]["model"]["id"]["name"],
        "gpt-model"
    );
}

fn lock_with_title(
    workspace: &mut Workspace,
    id: ConversationId,
    title: Option<&str>,
) -> jp_workspace::ConversationLock {
    let conversation = Conversation {
        title: title.map(str::to_owned),
        ..Default::default()
    };
    workspace.create_conversation_with_id(id, conversation, Arc::new(AppConfig::new_test()));
    let handle = workspace.acquire_conversation(&id).unwrap();
    workspace.test_lock(handle)
}

#[test]
fn resolve_new_title_uses_leading_heading() {
    assert_eq!(
        resolve_new_title(true, true, "# Fix the parser\n\nbody text"),
        NewTitle::FromHeading("Fix the parser".to_owned())
    );
}

#[test]
fn resolve_new_title_heading_wins_when_generation_disabled() {
    // The two flags are independent: `from_heading` still applies even
    // with LLM generation turned off.
    assert_eq!(
        resolve_new_title(true, false, "# Title"),
        NewTitle::FromHeading("Title".to_owned())
    );
}

#[test]
fn resolve_new_title_disabled_heading_falls_through_to_generation() {
    assert_eq!(
        resolve_new_title(false, true, "# Title"),
        NewTitle::Generate
    );
}

#[test]
fn resolve_new_title_no_heading_generates() {
    assert_eq!(
        resolve_new_title(true, true, "just a plain prompt"),
        NewTitle::Generate
    );
}

#[test]
fn resolve_new_title_skips_when_both_disabled() {
    assert_eq!(resolve_new_title(false, false, "# Title"), NewTitle::Skip);
    assert_eq!(
        resolve_new_title(true, false, "no heading here"),
        NewTitle::Skip
    );
}

#[test]
fn apply_title_override_no_title_clears_existing_title() {
    // `--no-title` should clear an inherited title (the
    // `--fork --no-title` case from PR #600 review): a forked
    // conversation inherits the source's title via
    // `fork_conversation`, and `--no-title` is supposed to leave
    // the run with no title at all.
    let mut workspace = Workspace::new("/tmp/test");
    let lock = lock_with_title(&mut workspace, make_id(1000), Some("inherited"));

    apply_title_override(&lock, None, true);

    assert_eq!(lock.metadata().title, None);
}

#[test]
fn apply_title_override_no_title_clears_resumed_title() {
    // `--no-title` is symmetric with `--title T`: both write the
    // user's intent into `metadata.title`, regardless of whether
    // the conversation is new, forked, or resumed.
    let mut workspace = Workspace::new("/tmp/test");
    let lock = lock_with_title(&mut workspace, make_id(1001), Some("existing"));

    apply_title_override(&lock, None, true);

    assert_eq!(lock.metadata().title, None);
}

#[test]
fn apply_title_override_title_overwrites_existing_title() {
    let mut workspace = Workspace::new("/tmp/test");
    let lock = lock_with_title(&mut workspace, make_id(1002), Some("old"));

    apply_title_override(&lock, Some("new"), false);

    assert_eq!(lock.metadata().title.as_deref(), Some("new"));
}

#[test]
fn apply_title_override_neither_flag_is_noop() {
    let mut workspace = Workspace::new("/tmp/test");
    let lock = lock_with_title(&mut workspace, make_id(1003), Some("keep"));

    apply_title_override(&lock, None, false);

    assert_eq!(lock.metadata().title.as_deref(), Some("keep"));
}

#[test]
fn no_title_does_not_persist_into_partial_config() {
    // Regression for the persistence concern in PR #600: routing
    // `--no-title` through `apply_cli_config` previously wrote
    // `conversation.title.generate.auto = Some(false)` into the
    // partial, which would then flow into the conversation's
    // `config_delta` via `get_config_delta_from_cli` and persist
    // for every future query on that conversation. The flag is
    // now strictly invocation-scoped, so the partial must be
    // untouched relative to a run without the flag.
    let base = PartialAppConfig::empty();

    let with_flag = Query {
        no_title: true,
        ..Default::default()
    }
    .apply_cli_config(None, base.clone(), None)
    .unwrap();
    let without_flag = Query::default().apply_cli_config(None, base, None).unwrap();

    assert_eq!(
        with_flag.conversation.title.generate.auto,
        without_flag.conversation.title.generate.auto,
    );
    assert_eq!(with_flag.conversation.title.generate.auto, None);
}

#[test]
fn echo_request_when_from_editor_or_replay() {
    // Editor-composed query: the editor took over the screen, so echo.
    assert!(Query::default().should_echo_request(true));

    // Plain inline query, no editor: the user already sees their input.
    assert!(!Query::default().should_echo_request(false));

    // Replay without an editor: the message comes from history and isn't
    // otherwise visible on the terminal, so it must be echoed.
    let replay = Query {
        replay: true,
        ..Default::default()
    };
    assert!(replay.should_echo_request(false));
}

#[test]
fn blockquote_prefixes_each_line() {
    assert_eq!(blockquote("hello"), "> hello");
    assert_eq!(blockquote("a\nb"), "> a\n> b");
    assert_eq!(blockquote("a\nb\nc"), "> a\n> b\n> c");
}

#[test]
fn blockquote_keeps_paragraph_breaks_with_bare_marker() {
    // Markdown continues a blockquote across a `>` line; an unprefixed
    // blank line would terminate it. The bare `>` (no trailing space)
    // also avoids editor trailing-whitespace warnings.
    assert_eq!(blockquote("a\n\nb"), "> a\n>\n> b");
}

#[test]
fn blockquote_trailing_newline_is_dropped_by_lines() {
    // `str::lines` drops the trailing terminator, so a string with and
    // without a trailing newline produce identical quotes.
    assert_eq!(blockquote("a\nb\n"), "> a\n> b");
}

#[test]
fn last_assistant_message_returns_most_recent_message() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("first question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("first answer"))
        .build()
        .unwrap();
    stream.start_turn("second question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("second answer"))
        .build()
        .unwrap();

    assert_eq!(last_assistant_message(&stream), Some("second answer"));
}

#[test]
fn last_assistant_message_skips_reasoning_after_message() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("the answer"))
        .add_chat_response(ChatResponse::reasoning("thinking after"))
        .build()
        .unwrap();

    // Reasoning is the most recent ChatResponse, but --quote wants the
    // assistant's spoken text, so the message wins.
    assert_eq!(last_assistant_message(&stream), Some("the answer"));
}

#[test]
fn last_assistant_message_returns_none_for_empty_stream() {
    let stream = ConversationStream::new_test();
    assert_eq!(last_assistant_message(&stream), None);
}

#[test]
fn last_assistant_message_returns_none_when_only_reasoning_present() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::reasoning("only thinking, no message yet"))
        .build()
        .unwrap();

    assert_eq!(last_assistant_message(&stream), None);
}
