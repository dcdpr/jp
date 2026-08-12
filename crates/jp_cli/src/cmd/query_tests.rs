use std::sync::Arc;

use assert_matches::assert_matches;
use chrono::{DateTime, Utc};
use clap::Parser as _;
use indexmap::IndexMap;
use jp_config::{
    AppConfig, PartialAppConfig, ToPartial,
    conversation::tool::{AllowToggle, Enable, PartialEnableConfig, PartialToolConfig},
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
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_term::width::display_width;
use jp_workspace::{
    ConversationHandle, Workspace,
    session::{Session, SessionId, SessionSource},
};
use relative_path::RelativePathBuf;
use serde_json::Value;
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals, KeyValueOrPath,
    cmd::target::{ConversationTarget, PickerFilter},
    config_pipeline::ConfigPipeline,
    signals::testing::detached_router,
};

fn make_partial_with_tools() -> PartialAppConfig {
    let mut partial = PartialAppConfig::default();
    partial.conversation.tools.tools = IndexMap::from_iter([
        ("implicitly_enabled_tool".into(), PartialToolConfig {
            enable: None,
            ..Default::default()
        }),
        ("explicitly_enabled_tool".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig::ON),
            ..Default::default()
        }),
        ("explicitly_disabled_tool".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig::OFF),
            ..Default::default()
        }),
        ("explicit_tool".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig {
                state: Some(false),
                allow_toggle: Some(AllowToggle::IfNamed),
            }),
            ..Default::default()
        }),
    ]);
    partial
}

/// Resolve a tool's effective [`Enable`] from a partial config: the per-tool
/// value over the global `*` defaults, then the hardcoded fallback.
fn effective(partial: &PartialAppConfig, name: &str) -> Enable {
    let defaults = partial
        .conversation
        .tools
        .defaults
        .enable
        .clone()
        .unwrap_or_default();
    partial.conversation.tools.tools[name]
        .enable
        .clone()
        .unwrap_or_default()
        .effective(&defaults)
}

/// Query input carrying `text` as the inline query, with `--quote` unset.
fn inline_query(text: &str) -> QueryInput {
    QueryInput {
        query: Some(vec![text.to_owned()]),
        ..Default::default()
    }
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
    let router = detached_router();

    turn_loop::run_turn_loop(
        Arc::clone(&provider),
        &model,
        cfg,
        &router,
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
        PendingStreamTrim::default(),
    )
    .await
    .unwrap();
}

#[test]
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

    assert!(effective(&partial, "implicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_enabled_tool").state);
    assert!(!effective(&partial, "explicitly_disabled_tool").state);
    assert!(!effective(&partial, "explicit_tool").state);

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

    assert!(!effective(&partial, "implicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_enabled_tool").state);
    assert!(!effective(&partial, "explicitly_disabled_tool").state);
    assert!(!effective(&partial, "explicit_tool").state);

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

    assert!(!effective(&partial, "implicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_disabled_tool").state);
    assert!(!effective(&partial, "explicit_tool").state);

    // Enable all tools -- if_named tools stay disabled and keep their policy.
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

    assert!(effective(&partial, "implicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_disabled_tool").state);
    let explicit = effective(&partial, "explicit_tool");
    assert!(
        !explicit.state,
        "explicit tools should NOT be enabled by --tools without arguments"
    );
    assert_eq!(explicit.allow_toggle, AllowToggle::IfNamed);

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

    assert!(!effective(&partial, "implicitly_enabled_tool").state);
    assert!(!effective(&partial, "explicitly_enabled_tool").state);
    assert!(!effective(&partial, "explicitly_disabled_tool").state);
    assert!(!effective(&partial, "explicit_tool").state);

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

    assert!(!effective(&partial, "implicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_disabled_tool").state);
    assert!(!effective(&partial, "explicit_tool").state);
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

    let explicit = effective(&partial, "explicit_tool");
    assert!(
        explicit.state,
        "explicit tools should be enabled when named specifically"
    );
    assert_eq!(explicit.allow_toggle, AllowToggle::IfNamed);
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

    assert!(effective(&partial, "implicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_disabled_tool").state);
    assert!(
        effective(&partial, "explicit_tool").state,
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

    assert!(effective(&partial, "implicitly_enabled_tool").state);
    let explicit = effective(&partial, "explicit_tool");
    assert!(
        !explicit.state,
        "bare --tools should not enable explicit tools"
    );
    assert_eq!(explicit.allow_toggle, AllowToggle::IfNamed);
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

    assert!(!effective(&partial, "implicitly_enabled_tool").state);
    assert!(!effective(&partial, "explicitly_enabled_tool").state);
    assert!(
        effective(&partial, "explicitly_disabled_tool").state,
        "named tool should be re-enabled after disable-all"
    );
    assert!(!effective(&partial, "explicit_tool").state);
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

    assert!(
        !effective(&partial, "implicitly_enabled_tool").state,
        "the carved-out tool should be disabled"
    );
    assert!(effective(&partial, "explicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_disabled_tool").state);
}

#[test]
fn test_interleaved_disable_all_then_enable_all() {
    // `--no-tools --tool`: both are bulk directives. Under the scope/policy
    // model a bulk directive only flips freely-toggleable (`any`) tools, so an
    // `if_named` tool keeps its policy and stays disabled across the sequence.
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

    assert!(effective(&partial, "implicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_enabled_tool").state);
    assert!(effective(&partial, "explicitly_disabled_tool").state);
    let explicit = effective(&partial, "explicit_tool");
    assert!(
        !explicit.state,
        "bulk directives must not flip an if_named tool"
    );
    assert_eq!(
        explicit.allow_toggle,
        AllowToggle::IfNamed,
        "the if_named policy is preserved across bulk directives"
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
    assert!(!effective(&partial, "implicitly_enabled_tool").state);
    assert!(!effective(&partial, "explicitly_enabled_tool").state);
    assert!(
        !effective(&partial, "explicitly_disabled_tool").state,
        "final disable should override the intermediate enable"
    );
    assert!(!effective(&partial, "explicit_tool").state);
}

#[test]
fn test_named_disable_of_locked_on_tool_errors() {
    // `describe_tools` is injected as locked-on; naming it for disable errors
    // with an allow_toggle-aware message.
    let err = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::Disable("describe_tools".into())]),
            ..Default::default()
        },
        None,
        make_partial_with_tools(),
        None,
    )
    .unwrap_err()
    .to_string();

    assert!(err.contains("describe_tools"), "unexpected error: {err}");
    assert!(err.contains("locked-on"), "unexpected error: {err}");
}

#[test]
fn test_named_enable_of_locked_off_tool_errors() {
    let mut partial = make_partial_with_tools();
    partial
        .conversation
        .tools
        .tools
        .insert("network".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig::LOCKED_OFF),
            ..Default::default()
        });

    let err = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::Enable("network".into())]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap_err()
    .to_string();

    assert!(err.contains("network"), "unexpected error: {err}");
    assert!(err.contains("locked-off"), "unexpected error: {err}");
}

#[test]
fn test_bulk_directives_skip_locked_tools() {
    let mut partial = make_partial_with_tools();
    partial
        .conversation
        .tools
        .tools
        .insert("network".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig::LOCKED_OFF),
            ..Default::default()
        });

    // Bulk enable then bulk disable: neither touches locked tools, neither
    // errors.
    let partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::EnableAll, ToolDirective::DisableAll]),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap();

    let net = effective(&partial, "network");
    assert!(
        !net.state,
        "locked-off tool stays off through bulk directives"
    );
    assert!(net.is_locked());
    // The injected locked-on builtin stays on.
    assert!(effective(&partial, "describe_tools").state);
}

#[test]
fn test_sticky_tool_named_disable_and_bulk_skip() {
    let make = || {
        let mut partial = PartialAppConfig::default();
        partial
            .conversation
            .tools
            .tools
            .insert("ask_user".into(), PartialToolConfig {
                enable: Some(PartialEnableConfig {
                    state: Some(true),
                    allow_toggle: Some(AllowToggle::IfNamed),
                }),
                ..Default::default()
            });
        partial
    };

    // A named disable flips a sticky tool off.
    let partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::Disable("ask_user".into())]),
            ..Default::default()
        },
        None,
        make(),
        None,
    )
    .unwrap();
    assert!(!effective(&partial, "ask_user").state);

    // A bulk disable leaves a sticky tool on (`if_named` rejects bulk).
    let partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::DisableAll]),
            ..Default::default()
        },
        None,
        make(),
        None,
    )
    .unwrap();
    let sticky = effective(&partial, "ask_user");
    assert!(sticky.state, "bulk disable must not flip an if_named tool");
    assert_eq!(sticky.allow_toggle, AllowToggle::IfNamed);
}

#[test]
fn test_tool_use_accepts_tool_enabled_via_defaults() {
    // A tool with no per-tool `enable` is enabled via the default-on fallback,
    // so `-u <name>` accepts it.
    let mut partial = PartialAppConfig::default();
    partial
        .conversation
        .tools
        .tools
        .insert("fs_read".into(), PartialToolConfig {
            source: Some(ToolSource::Local { tool: None }),
            ..Default::default()
        });

    let result = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_use: Some(Some("fs_read".into())),
            ..Default::default()
        },
        None,
        partial,
        None,
    );
    assert!(result.is_ok(), "{:?}", result.err());
}

#[test]
fn test_tool_use_accepts_locked_on_builtin() {
    // `describe_tools` (locked-on) is force-selectable via `-u`.
    let result = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_use: Some(Some("describe_tools".into())),
            ..Default::default()
        },
        None,
        make_partial_with_tools(),
        None,
    );
    assert!(result.is_ok(), "{:?}", result.err());
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
        input: inline_query("is this thing on?"),
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
        input: inline_query("are you there?"),
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
        input: inline_query("plain query"),
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
fn echo_request_unless_inline() {
    // Editor-composed query: the editor took over the screen, so echo.
    assert!(Query::default().should_echo_request(QuerySource::Editor));

    // Plain inline query, no editor: the user already sees their input.
    assert!(!Query::default().should_echo_request(QuerySource::Inline));

    // Synthesized query (`--no-edit` without a query): the user never typed
    // or saw the resulting message, so it must be echoed.
    assert!(Query::default().should_echo_request(QuerySource::Synthesized));

    // Replay without an editor: the message comes from history and isn't
    // otherwise visible on the terminal, so it must be echoed.
    let replay = Query {
        replay: true,
        ..Default::default()
    };
    assert!(replay.should_echo_request(QuerySource::Inline));
}

#[test]
fn edit_message_synthesizes_when_no_edit_without_query() {
    let config = AppConfig::new_test();
    let root = Utf8Path::new("/tmp");
    let query = Query {
        no_edit: true,
        ..Default::default()
    };

    // Empty request and empty stream: a default "continue" message is
    // synthesized, so the caller must echo it.
    let mut request = ChatRequest::default();
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let (source, partial) = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            &config,
            root,
        )
        .unwrap();
    assert_eq!(source, QuerySource::Synthesized);
    assert_eq!(request.content, "continue");
    assert!(partial.is_empty());

    // Empty request with the stream's trailing event being a chat request:
    // that request is peeked and re-sent verbatim, also synthesized.
    let mut request = ChatRequest::default();
    let mut stream = ConversationStream::new_test();
    stream.start_turn("earlier text");
    let mut pending_trim = PendingStreamTrim::default();
    let (source, _) = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            &config,
            root,
        )
        .unwrap();
    assert_eq!(source, QuerySource::Synthesized);
    assert_eq!(request.content, "earlier text");
    // The trailing request is not popped here; its removal is deferred to the
    // turn-start commit point via `pending_trim`, so the stream is untouched.
    assert!(pending_trim.pop_request);
    assert!(stream.pop_if(ConversationEvent::is_chat_request).is_some());
}

#[test]
fn edit_message_quote_without_editor_is_synthesized() {
    // `--quote --no-edit`: `build_conversation` seeds the request with the
    // quoted assistant message before `edit_message` runs, so the request is
    // non-empty here even though the user never typed or saw the final text.
    // It must be classified as synthesized (and therefore echoed), not
    // inline.
    let config = AppConfig::new_test();
    let query = Query {
        input: QueryInput {
            quote: Some(true),
            ..Default::default()
        },
        no_edit: true,
        ..Default::default()
    };

    let mut request = ChatRequest::from(" >  quoted reply");
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let (source, partial) = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            false,
            &config,
            Utf8Path::new("/tmp"),
        )
        .unwrap();
    assert_eq!(source, QuerySource::Synthesized);
    // The seeded content is sent as-is.
    assert_eq!(request.content, " >  quoted reply");
    assert!(partial.is_empty());
}

#[test]
fn edit_message_skips_editor_when_no_edit_with_piped_stdin() {
    // Regression: `--no-edit` must skip the editor even when piped stdin
    // content makes the request non-empty. Previously the explicit override
    // was only checked when the request was still empty, so `--no-edit` was
    // silently ignored whenever stdin was piped without a query argument.
    let config = AppConfig::new_test();
    let root = Utf8Path::new("/tmp");
    let query = Query {
        no_edit: true,
        ..Default::default()
    };

    let mut request = ChatRequest::from("hi");
    let stream = ConversationStream::new_test();
    let mut pending_trim = PendingStreamTrim::default();
    let (source, partial) = query
        .edit_message(
            &mut request,
            &stream,
            &mut pending_trim,
            true,
            &config,
            root,
        )
        .unwrap();

    assert_eq!(source, QuerySource::Inline);
    assert_eq!(request.content, "hi");
    assert!(partial.is_empty());
}

#[test]
fn picker_new_item_gated_by_new_incompatible_flags() {
    // The picker may only offer "start a new conversation" when `--new` would
    // itself be a legal flag. `--fork` and `--replay` both conflict with
    // `--new` (and still reach the picker), so choosing the item would
    // otherwise silently drop the fork/replay request and manufacture a state
    // clap rejects at parse time.
    assert!(Query::default().allows_new_from_picker());

    let fork = Query {
        fork: Some(None),
        ..Default::default()
    };
    assert!(!fork.allows_new_from_picker());

    let replay = Query {
        replay: true,
        ..Default::default()
    };
    assert!(!replay.allows_new_from_picker());
}

#[test]
fn picker_new_item_gated_by_bare_id_flag() {
    // Bare `jp query --id` parses to an empty value (`FlagIds` sets
    // `default_missing_value = ""`), which becomes the interactive picker.
    // Since `--new` conflicts with the `id` arg, the picker must not offer the
    // synthetic "new" item: choosing it would drop the target and manufacture a
    // `--new --id` state clap rejects at parse time.
    let bare_id = Query {
        target: FlagIds::from_targets(vec![ConversationTarget::Picker(PickerFilter::default())]),
        ..Default::default()
    };
    assert!(!bare_id.allows_new_from_picker());
}

/// The query words a parse produced, for comparison against a static slice.
fn query_words(query: &Query) -> &[String] {
    query.input.query.as_deref().unwrap_or_default()
}

#[test]
fn bare_quote_uses_the_blockquote_prefix() {
    let query = parse_query(&["--quote"]).unwrap();
    assert_eq!(query.input.quote, Some(true));
    assert!(query_words(&query).is_empty());
}

#[test]
fn quote_true_is_the_same_as_a_bare_quote() {
    let query = parse_query(&["--quote=true"]).unwrap();
    assert_eq!(query.input.quote, Some(true));
    assert!(query_words(&query).is_empty());
}

#[test]
fn quote_false_still_quotes_but_drops_the_prefix() {
    let query = parse_query(&["--quote=false"]).unwrap();
    assert_eq!(query.input.quote, Some(false));
    assert!(query_words(&query).is_empty());
}

#[test]
fn quote_is_absent_when_not_given() {
    let query = parse_query(&[]).unwrap();
    assert_eq!(query.input.quote, None);
}

#[test]
fn quote_takes_an_unattached_bool() {
    let query = parse_query(&["--quote", "false"]).unwrap();
    assert_eq!(query.input.quote, Some(false));
    assert!(query_words(&query).is_empty());

    let query = parse_query(&["--quote", "true"]).unwrap();
    assert_eq!(query.input.quote, Some(true));
    assert!(query_words(&query).is_empty());
}

#[test]
fn quote_takes_an_unattached_bool_ahead_of_the_query() {
    let query = parse_query(&["--quote", "false", "and", "now?"]).unwrap();
    assert_eq!(query.input.quote, Some(false));
    assert_eq!(query_words(&query), ["and".to_owned(), "now?".to_owned()]);
}

#[test]
fn quote_does_not_swallow_a_following_flag() {
    let query = parse_query(&["--quote", "--model", "foo"]).unwrap();
    assert_eq!(query.input.quote, Some(true));
    assert_eq!(query.model.as_deref(), Some("foo"));
}

#[test]
fn quote_does_not_swallow_the_positional_query() {
    let query = parse_query(&["--quote", "what about X?"]).unwrap();
    assert_eq!(query.input.quote, Some(true));
    assert_eq!(query_words(&query), ["what about X?".to_owned()]);
}

#[test]
fn quote_only_takes_the_word_directly_after_it() {
    // `false` is the query here: it sits before the flag, so it was never
    // offered as the flag's value.
    let query = parse_query(&["false", "--quote"]).unwrap();
    assert_eq!(query.input.quote, Some(true));
    assert_eq!(query_words(&query), ["false".to_owned()]);

    // Same when another flag sits between the two.
    let query = parse_query(&["--quote", "--model", "foo", "false"]).unwrap();
    assert_eq!(query.input.quote, Some(true));
    assert_eq!(query_words(&query), ["false".to_owned()]);
}

#[test]
fn a_double_dash_shields_a_bool_from_quote() {
    let query = parse_query(&["--quote", "--", "false"]).unwrap();
    assert_eq!(query.input.quote, Some(true));
    assert_eq!(query_words(&query), ["false".to_owned()]);
}

#[test]
fn words_before_and_after_a_double_dash_form_one_query() {
    let query = parse_query(&["why", "--", "--model", "foo"]).unwrap();
    assert_eq!(query.input.quote, None);
    assert_eq!(query_words(&query), [
        "why".to_owned(),
        "--model".to_owned(),
        "foo".to_owned()
    ]);
}

#[test]
fn quote_with_an_attached_value_leaves_a_following_bool_in_the_query() {
    // `--quote=false` has its value already, so the `true` after it is query
    // text. Without the attached/bare distinction both forms would look
    // identical here, since clap records the same index for either.
    let query = parse_query(&["--quote=false", "true"]).unwrap();
    assert_eq!(query.input.quote, Some(false));
    assert_eq!(query_words(&query), ["true".to_owned()]);
}

#[test]
fn quote_rejects_an_attached_non_boolean_value() {
    assert!(parse_query(&["--quote=foo"]).is_err());
}

/// A stream whose last assistant message is a two-line reply.
fn stream_with_assistant_reply() -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("line one\nline two"))
        .build()
        .unwrap();
    stream
}

#[test]
fn quote_false_seeds_the_message_verbatim() {
    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(
        &mut request,
        &stream_with_assistant_reply(),
        false
    ));

    // The trailing blank line separates the seed from the reply the user is
    // about to type below it.
    assert_eq!(request.content, "line one\nline two\n\n");
}

#[test]
fn quote_true_seeds_the_message_as_a_blockquote() {
    let mut request = ChatRequest::default();
    assert!(seed_quoted_reply(
        &mut request,
        &stream_with_assistant_reply(),
        true
    ));

    assert_eq!(request.content, "> line one\n> line two\n\n");
}

#[test]
fn quote_seeds_above_an_already_composed_request() {
    let mut request = ChatRequest::from("and what about X?");
    assert!(seed_quoted_reply(
        &mut request,
        &stream_with_assistant_reply(),
        false
    ));

    assert_eq!(request.content, "line one\nline two\n\nand what about X?");
}

#[test]
fn quote_leaves_the_request_untouched_without_an_assistant_message() {
    let mut request = ChatRequest::from("only my words");
    assert!(!seed_quoted_reply(
        &mut request,
        &ConversationStream::new_test(),
        true
    ));

    assert_eq!(request.content, "only my words");
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

/// Count the `TurnStart` events in a stream.
fn turn_start_count(stream: &ConversationStream) -> usize {
    stream.iter().filter(|e| e.event.is_turn_start()).count()
}

/// Assert that no `TurnStart` is immediately followed by another `TurnStart`
/// (an empty middle turn).
fn assert_no_adjacent_turn_starts(stream: &ConversationStream) {
    let mut previous_was_turn_start = false;
    for e in stream.iter() {
        assert!(
            !(previous_was_turn_start && e.event.is_turn_start()),
            "stream contains adjacent TurnStart events (empty middle turn)"
        );
        previous_was_turn_start = e.event.is_turn_start();
    }
}

#[test]
fn pending_trim_replay_removes_stale_turn_start() {
    // Multi-turn conversation: the stale `TurnStart` sits *after* the first
    // `ChatRequest`, where `sanitize`'s `normalize_turn_starts` would not
    // collapse it.
    let mut stream = ConversationStream::new_test();
    stream.start_turn("first question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("first answer"))
        .build()
        .unwrap();
    stream.start_turn("second question");

    let trim = PendingStreamTrim {
        replay_turn: true,
        pop_request: false,
    };
    trim.apply(&mut stream);

    // The replayed request re-enters the stream as a fresh turn.
    stream.start_turn("second question, revised");

    assert_eq!(
        turn_start_count(&stream),
        2,
        "replay must replace the trimmed turn, not open an extra one"
    );
    assert_no_adjacent_turn_starts(&stream);
}

#[test]
fn pending_trim_pop_request_removes_stale_turn_start() {
    // Bare `--no-edit` replay: the last turn holds only its request.
    let mut stream = ConversationStream::new_test();
    stream.start_turn("first question");
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("first answer"))
        .build()
        .unwrap();
    stream.start_turn("replayed question");

    let trim = PendingStreamTrim {
        replay_turn: false,
        pop_request: true,
    };
    trim.apply(&mut stream);

    stream.start_turn("replayed question");

    assert_eq!(
        turn_start_count(&stream),
        2,
        "pop_request must replace the trimmed turn, not open an extra one"
    );
    assert_no_adjacent_turn_starts(&stream);
}

#[test]
fn pending_trim_default_is_noop() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn("question");
    let before = stream.len();

    PendingStreamTrim::default().apply(&mut stream);

    assert_eq!(
        stream.len(),
        before,
        "a default PendingStreamTrim must not mutate the stream"
    );
}

#[test]
fn mcp_startup_status_single_server() {
    assert_eq!(
        mcp_startup_status(&[McpServerId::new("bookworm")]),
        "MCP server bookworm"
    );
}

#[test]
fn mcp_startup_status_multiple_servers() {
    assert_eq!(
        mcp_startup_status(&[McpServerId::new("bookworm"), McpServerId::new("grizzly")]),
        "2 MCP servers (bookworm, grizzly)"
    );
}

/// Timer settings that render immediately, so tests don't wait out a delay.
fn immediate_mcp_startup_config() -> McpStartupConfig {
    McpStartupConfig {
        show: true,
        delay_secs: 0,
        interval_ms: 10,
    }
}

#[tokio::test]
async fn await_mcp_servers_drains_all_startups() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);

    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async { Ok(McpServerId::new("bookworm")) });
    joins.spawn(async { Ok(McpServerId::new("grizzly")) });
    let startup = StartupSet {
        joins,
        pending: vec![McpServerId::new("bookworm"), McpServerId::new("grizzly")],
    };

    await_mcp_servers(
        startup,
        immediate_mcp_startup_config(),
        Arc::new(printer),
        false,
        None,
    )
    .await
    .expect("all startups succeed");
}

#[tokio::test]
async fn await_mcp_servers_propagates_startup_error() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);

    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async { Err(jp_mcp::Error::UnknownServer(McpServerId::new("bookworm"))) });
    let startup = StartupSet {
        joins,
        pending: vec![McpServerId::new("bookworm")],
    };

    let error = await_mcp_servers(
        startup,
        immediate_mcp_startup_config(),
        Arc::new(printer),
        false,
        None,
    )
    .await
    .expect_err("a failed required server must fail the wait");

    assert_eq!(error.message.as_deref(), Some("MCP error"));
}

#[tokio::test(flavor = "multi_thread")]
async fn await_mcp_servers_shows_and_clears_timer_line() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);

    // Hold the startup window open until the test releases it, so the timer
    // is guaranteed to tick while the server is still "starting".
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async move {
        release_rx.await.ok();
        Ok(McpServerId::new("bookworm"))
    });
    let startup = StartupSet {
        joins,
        pending: vec![McpServerId::new("bookworm")],
    };

    let wait = tokio::spawn(await_mcp_servers(
        startup,
        immediate_mcp_startup_config(),
        printer.clone(),
        true,
        None,
    ));

    // Let a few ticks land before releasing the startup.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    release_tx.send(()).expect("wait task is still running");
    wait.await
        .expect("task did not panic")
        .expect("startup succeeds");
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.contains("⏱ Starting MCP server bookworm…"),
        "timer line should name the pending server.\nChrome:\n{chrome}"
    );
    assert!(
        chrome.ends_with("\r\x1b[K"),
        "finishing the wait must leave the line cleared.\nChrome:\n{chrome}"
    );
}

/// Poll `err` until `needle` appears, failing after a hard timeout.
///
/// Synchronizes on the rendered output instead of a fixed sleep: the timer
/// writes frames from its own task, so tests wait for the frame to land rather
/// than guessing how long that takes.
async fn wait_for_frame(err: &SharedBuffer, needle: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !err.lock().contains(needle) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("frame {needle:?} never rendered"));
}

/// Drives the aggregate redraw: two servers start, one finishes while the other
/// is still pending, then the second finishes.
/// The line must go from both names, to the survivor alone, to cleared.
#[tokio::test(flavor = "multi_thread")]
async fn await_mcp_servers_redraws_as_servers_finish() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);

    // Two independently-released tasks: releasing `bookworm` first makes
    // `grizzly` the deterministic survivor of the mid-drain redraw.
    let (bookworm_tx, bookworm_rx) = tokio::sync::oneshot::channel::<()>();
    let (grizzly_tx, grizzly_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async move {
        bookworm_rx.await.ok();
        Ok(McpServerId::new("bookworm"))
    });
    joins.spawn(async move {
        grizzly_rx.await.ok();
        Ok(McpServerId::new("grizzly"))
    });
    let startup = StartupSet {
        joins,
        pending: vec![McpServerId::new("bookworm"), McpServerId::new("grizzly")],
    };

    let wait = tokio::spawn(await_mcp_servers(
        startup,
        immediate_mcp_startup_config(),
        printer.clone(),
        true,
        None,
    ));

    // Advance on the rendered frames, not the clock: wait until each frame is
    // actually in the buffer before releasing the next server, so a slow timer
    // task can't make the release outrun the redraw it's supposed to observe.
    wait_for_frame(&err, "2 MCP servers (bookworm, grizzly)").await;
    bookworm_tx.send(()).expect("wait task is still running");
    wait_for_frame(&err, "MCP server grizzly…").await;
    grizzly_tx.send(()).expect("wait task is still running");
    wait.await
        .expect("task did not panic")
        .expect("all startups succeed");
    printer.flush();

    let chrome = err.lock();
    let both = chrome
        .find("2 MCP servers (bookworm, grizzly)")
        .expect("the aggregate two-server frame must render first");
    let survivor = chrome
        .find("MCP server grizzly…")
        .expect("the survivor-only frame must render after bookworm finishes");
    assert!(
        both < survivor,
        "the two-server frame must precede the survivor-only frame.\nChrome:\n{chrome}"
    );
    assert!(
        !chrome.contains("MCP server bookworm…"),
        "bookworm was never the sole pending server; it must not render alone.\nChrome:\n{chrome}"
    );
    assert!(
        chrome.ends_with("\r\x1b[K"),
        "finishing the wait must leave the line cleared.\nChrome:\n{chrome}"
    );
}

#[test]
fn mcp_startup_line_renders_full_when_it_fits() {
    assert_eq!(
        mcp_startup_line(4.2, Some("MCP server bookworm"), Some(80)),
        "\r\x1b[K⏱ Starting MCP server bookworm… 4.2s"
    );
    // Unknown width leaves the line unbounded.
    assert_eq!(
        mcp_startup_line(4.2, Some("MCP server bookworm"), None),
        "\r\x1b[K⏱ Starting MCP server bookworm… 4.2s"
    );
}

// A long server list forced to truncate must keep the elapsed-time suffix: the
// whole point of the line is the moving timer, so truncation has to fall on the
// server list, not the `Ns` tail. Testing the pure formatter at a fixed `secs`
// pins the invariant without depending on when the timer task first ticks.
#[test]
fn mcp_startup_line_truncation_preserves_timer_suffix() {
    let long = "MCP server bookworm-with-a-very-long-descriptive-server-name";
    let line = mcp_startup_line(12.3, Some(long), Some(30));

    assert!(line.ends_with(" 12.3s"), "suffix must survive: {line:?}");
    assert!(line.contains('…'), "server list must truncate: {line:?}");
    // The visible text (control prefix stripped) must fit the declared width.
    let visible = line.strip_prefix("\r\x1b[K").expect("control prefix");
    assert!(display_width(visible) <= 30, "must fit width: {line:?}");
}

// A terminal too narrow for even the prefix and suffix still keeps a moving
// timer rather than a static stub.
#[test]
fn mcp_startup_line_ultra_narrow_keeps_bounded_timer() {
    let line = mcp_startup_line(7.0, Some("MCP server bookworm"), Some(6));

    let visible = line.strip_prefix("\r\x1b[K").expect("control prefix");
    assert!(display_width(visible) <= 6, "must fit width: {line:?}");
    assert!(visible.contains("7.0s"), "timer must survive: {line:?}");
}

#[test]
fn arg_file_path_recognizes_sigil() {
    assert_eq!(arg_file_path("@notes.md"), Some("notes.md"));
    assert_eq!(arg_file_path("@~/notes.md"), Some("~/notes.md"));
    assert_eq!(arg_file_path("notes.md"), None);
    assert_eq!(arg_file_path(""), None);
}

// A bare `@` is ordinary prose, not a reference to the empty path. Reading it
// as a path is what made `jp q ... drop the @ entirely ...` fail.
#[test]
fn arg_file_path_ignores_bare_sigil() {
    assert_eq!(arg_file_path("@"), None);
    assert_eq!(arg_file_path("@ "), None);
    assert_eq!(arg_file_path("@\t"), None);
}

#[test]
fn query_file_path_only_for_single_value_query() {
    let one = ["@notes.md".to_owned()];
    assert_eq!(query_file_path(&one), Some("notes.md"));

    let trailing = ["hello".to_owned(), "@notes.md".to_owned()];
    assert_eq!(query_file_path(&trailing), None);

    let leading = ["@notes.md".to_owned(), "extra".to_owned()];
    assert_eq!(query_file_path(&leading), None);

    let plain = ["hello".to_owned()];
    assert_eq!(query_file_path(&plain), None);

    assert_eq!(query_file_path(&[]), None);
}

#[test]
fn read_arg_file_returns_file_contents() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n\nbody\n").unwrap();

    assert_eq!(read_arg_file(path.as_str()).unwrap(), "# Notes\n\nbody\n");
}

/// Host for [`Query`]'s arguments, so tests can drive the real clap parser
/// rather than constructing a [`Query`] the CLI could never produce.
#[derive(clap::Parser)]
struct QueryArgs {
    #[command(flatten)]
    query: Query,
}

/// Parse `jp query <args>`, always with `--no-edit` so no editor opens.
fn parse_query(args: &[&str]) -> std::result::Result<Query, clap::Error> {
    let argv = ["query", "--no-edit"]
        .into_iter()
        .chain(args.iter().copied());

    QueryArgs::try_parse_from(argv).map(|parsed| parsed.query)
}

/// Build the request for `args`, with no piped stdin and an empty stream.
///
/// Runs the same two steps `run` does: resolve the query, then compose it.
fn built_request(args: &[&str]) -> String {
    built_request_against(args, &ConversationStream::new_test())
}

/// Build the request for `args` against `stream`, with no piped stdin.
fn built_request_against(args: &[&str], stream: &ConversationStream) -> String {
    let query = parse_query(args).unwrap();
    let resolved = query.resolve_query().unwrap();

    query
        .build_conversation(
            "",
            resolved.as_deref(),
            stream,
            &AppConfig::new_test(),
            Utf8Path::new("/tmp"),
        )
        .unwrap()
        .chat_request
        .expect("non-empty request")
        .content
}

#[test]
fn build_conversation_seeds_a_verbatim_quote_above_the_query() {
    let built = built_request_against(
        &["--quote=false", "and", "what", "about", "X?"],
        &stream_with_assistant_reply(),
    );

    assert_eq!(built, "line one\nline two\n\nand what about X?");
}

#[test]
fn build_conversation_seeds_a_blockquoted_quote_above_the_query() {
    let built = built_request_against(
        &["--quote", "and", "what", "about", "X?"],
        &stream_with_assistant_reply(),
    );

    assert_eq!(built, "> line one\n> line two\n\nand what about X?");
}

#[test]
fn build_conversation_reads_single_at_path_query() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n\nbody\n").unwrap();

    assert_eq!(
        built_request(&[format!("@{path}").as_str()]),
        "# Notes\n\nbody\n"
    );
}

// A bare `@` is the whole query: there is no path after the sigil, so it is
// text. Reading it as the empty path is what aborted the query.
#[test]
fn build_conversation_keeps_lone_bare_sigil() {
    assert_eq!(built_request(&["@"]), "@");
}

// The reported failure: a bare `@` mid-sentence was read as the empty path and
// aborted the query before it was ever sent.
#[test]
fn build_conversation_keeps_bare_sigil_in_prose() {
    assert_eq!(
        built_request(&["we", "should", "drop", "the", "@", "entirely"]),
        "we should drop the @ entirely"
    );
}

// An `@path` that names a real file is still ordinary text when it is one word
// of a longer query: only a single-value query reads from disk.
#[test]
fn build_conversation_keeps_at_path_word_in_multi_word_query() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "file contents").unwrap();

    assert_eq!(
        built_request(&["see", format!("@{path}").as_str()]),
        format!("see @{path}")
    );
    // Leading position too: it is the value count that decides, not where the
    // sigil sits.
    assert_eq!(
        built_request(&[format!("@{path}").as_str(), "is", "stale"]),
        format!("@{path} is stale")
    );
}

#[test]
fn build_conversation_prepends_query_to_piped_stdin() {
    let built = parse_query(&["look", "at", "this"])
        .unwrap()
        .build_conversation(
            "piped payload",
            Some("look at this"),
            &ConversationStream::new_test(),
            &AppConfig::new_test(),
            Utf8Path::new("/tmp"),
        )
        .unwrap();

    assert_eq!(
        built.chat_request.unwrap().content,
        "look at this\n\npiped payload"
    );
}

// A query naming an unreadable file must fail before any conversation or
// session state is touched. The read used to happen after the conversation was
// created and recorded as the session's active one, so a typo'd path left an
// empty conversation behind that the next query silently targeted.
#[test]
fn run_missing_at_path_query_leaves_conversation_and_session_untouched() {
    let dir = camino_tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.md");

    let session = Session {
        id: SessionId::new("jp-cli-query-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let workspace = Workspace::new("/tmp/jp-cli-query-test");
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        Some(session.clone()),
        printer,
    );

    let query = parse_query(&["--new", &format!("@{missing}")]).unwrap();
    let result = Runtime::new()
        .unwrap()
        .block_on(query.run(&mut ctx, None, false));

    let Err(error) = result else {
        panic!("a query naming a missing file must fail");
    };
    // Pin that it failed on the unreadable path, not on something the test
    // environment happens to be missing further down `run`.
    assert_eq!(
        error
            .metadata
            .iter()
            .find(|(key, _)| key == "path")
            .map(|(_, value)| value),
        Some(&Value::from(missing.as_str())),
    );

    assert_eq!(ctx.workspace.conversations().count(), 0);
    assert_eq!(ctx.workspace.session_active_conversation(&session), None);
}

#[test]
fn resolve_query_missing_at_path_errors() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.md");
    let query = parse_query(&[&format!("@{path}")]).unwrap();

    let Err(error) = query.resolve_query() else {
        panic!("a query naming a missing file must fail, not be sent verbatim");
    };
    assert_matches!(&error, Error::ArgFile { path: p, .. } if p == path.as_str());
}

#[test]
fn read_arg_file_error_names_the_path() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.md");

    let error = read_arg_file(path.as_str()).unwrap_err();

    assert_matches!(&error, Error::ArgFile { path: p, .. } if p == path.as_str());
    // The OS-supplied cause differs per platform, so pin the part we own: the
    // path is in the message clap renders, which is what "IO error" dropped.
    assert!(
        error
            .to_string()
            .starts_with(&format!("cannot read '{path}': ")),
        "unexpected message: {error}"
    );
}
