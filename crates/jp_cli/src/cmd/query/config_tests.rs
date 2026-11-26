use std::sync::Arc;

use camino::Utf8Path;
use camino_tempfile::Utf8TempDir;
use indexmap::IndexMap;
use jp_config::{
    ToPartial,
    assistant::sections::PartialSectionConfig,
    conversation::{
        attachment::PartialAttachmentConfig,
        tool::{AllowToggle, PartialCommandConfigOrString, ResultMode, RunMode, ToolSource},
    },
    model::id::{ModelIdConfig, PartialModelIdConfig},
    plugins::command::RunPolicy,
    types::vec::MergeableVec,
    util::build,
};
use jp_conversation::{Conversation, ConversationId, event::ChatRequest};
use jp_inquire::prompt::MockPromptBackend;
use jp_llm::{DynProvider, provider::mock::MockProvider};
use jp_mcp::server::tool_definitions;
use jp_printer::{OutputFormat, Printer};
use jp_tool::InvocationContext;
use relative_path::RelativePathBuf;
use serde_json::Value;
use url::Url;

use super::*;
use crate::{
    Cli, KeyValueOrPath, Resolved,
    cmd::query::{
        PendingStreamTrim,
        interrupt::TurnInterrupts,
        tool::executor::{ExecutorSource, tests::test_support::TestExecutorSource},
        turn_loop,
    },
    config_pipeline::ConfigPipeline,
    resolve_config,
    signals::tests::test_support::detached_router,
};

fn make_partial_with_tools() -> PartialAppConfig {
    let mut partial = PartialAppConfig::default();
    *partial.conversation.tools.tools = IndexMap::from_iter([
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

/// `text` as the inline query.
#[expect(
    clippy::unnecessary_wraps,
    reason = "stands in for the `Option` a query holds"
)]
fn inline_query(text: &str) -> Option<Vec<String>> {
    Some(vec![text.to_owned()])
}

/// Helper to build directives from a list.
fn directives(ds: Vec<ToolDirective>) -> Vec<ToolDirective> {
    ds
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
    Box::new(TestExecutorSource::new())
}

fn build_query_config(
    workspace: &Workspace,
    base: PartialAppConfig,
    cfg_args: &[KeyValueOrPath],
    query: &Query,
    handle: Option<&ConversationHandle>,
) -> AppConfig {
    let pipeline = ConfigPipeline::new(cfg_args, Some(workspace), None, || Ok(base)).unwrap();

    let conversation_partial = handle
        .map(|handle| conversation_config(workspace, PartialAppConfig::default(), handle).unwrap());

    let mut partial = match conversation_partial {
        Some((conversation_partial, unsets)) => {
            pipeline.partial_with_conversation(conversation_partial, &unsets)
        }
        None => pipeline.partial_without_conversation(),
    }
    .unwrap();

    partial = query
        .apply_cli_config(Some(workspace), partial, None)
        .unwrap();

    build(partial).unwrap()
}

async fn run_mock_turn(
    cfg: &AppConfig,
    lock: &jp_workspace::ConversationLock,
    prompt: &str,
    response: &str,
) {
    let provider: Arc<dyn DynProvider> = Arc::new(MockProvider::with_message(response));
    let model = provider
        .model_details(&cfg.assistant.model.id.resolved().name)
        .await
        .unwrap();
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let router = detached_router();

    turn_loop::run_turn_loop(
        Arc::clone(&provider),
        &model,
        cfg,
        &router,
        Utf8Path::new("/tmp"),
        InvocationContext::default(),
        false, // interactive
        &[],
        lock,
        jp_config::assistant::tool_choice::ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        tool::ToolCoordinator::new(cfg.conversation.tools.clone(), empty_executor_source()),
        ChatRequest::from(prompt),
        PendingStreamTrim::default(),
        router.turn_interrupt(),
        TurnInterrupts::none(),
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

    assert_eq!(
        err,
        "cannot enable `network`: this tool is configured as locked-off"
    );
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
            tool_use: Some(Some("fs_read".to_owned())),
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
            tool_use: Some(Some("describe_tools".to_owned())),
            ..Default::default()
        },
        None,
        make_partial_with_tools(),
        None,
    );
    assert!(result.is_ok(), "{:?}", result.err());
}

#[test]
fn test_tool_use_forces_a_disabled_tool_for_one_turn() {
    // `jp q -u explicitly_disabled_tool` runs that tool without a second flag:
    // `tool_definitions` keeps a forced tool even while it is disabled, so the
    // choice alone is enough and nothing has to be enabled in the config.
    let query = Query {
        tool_use: Some(Some("explicitly_disabled_tool".to_owned())),
        ..Default::default()
    };

    let partial =
        IntoPartialAppConfig::apply_cli_config(&query, None, make_partial_with_tools(), None)
            .unwrap();

    assert_eq!(
        query.effective_tool_choice(&AppConfig::new_test()),
        ToolChoice::Function("explicitly_disabled_tool".into())
    );
    assert!(
        !effective(&partial, "explicitly_disabled_tool").state,
        "the force is invocation-scoped: the tool stays disabled in the config"
    );
}

/// `jp q -a` refuses a retired MCP resource URI the same way `jp attachment
/// add` does, rather than storing one that fails every later query.
#[test]
fn query_attachment_flag_refuses_a_retired_mcp_attachment() {
    let uri = "mcp+github-mcp-server+repo://owner/name";
    let query = Query {
        attachments: vec![AttachmentUrlOrPath::Url(Url::parse(uri).unwrap())],
        ..Default::default()
    };

    let error =
        IntoPartialAppConfig::apply_cli_config(&query, None, PartialAppConfig::default(), None)
            .expect_err("a retired attachment is refused");

    assert_eq!(error.to_string(), format!("Attachment error for {uri}"));
    assert_eq!(
        std::error::Error::source(error.as_ref())
            .map(ToString::to_string)
            .as_deref(),
        Some(
            "MCP resource attachments are retired and can no longer be added: \
             `mcp+github-mcp-server+repo://owner/name`"
        )
    );
}

#[test]
fn test_tool_use_accepts_an_if_named_tool() {
    // `explicit_tool` is off with `allow_toggle = if_named`. A named force is
    // exactly what its policy permits, so `-u` is accepted rather than refused.
    let query = Query {
        tool_use: Some(Some("explicit_tool".to_owned())),
        ..Default::default()
    };

    IntoPartialAppConfig::apply_cli_config(&query, None, make_partial_with_tools(), None)
        .expect("an if_named tool accepts a named force");

    assert_eq!(
        query.effective_tool_choice(&AppConfig::new_test()),
        ToolChoice::Function("explicit_tool".into())
    );
}

#[test]
fn test_tool_choice_without_a_flag_comes_from_config() {
    // The other side of `effective_tool_choice`: with no `-u`/`-U`, the
    // conversation's configured choice is what applies.
    let mut cfg = AppConfig::new_test();
    cfg.assistant.tool_choice = ToolChoice::Required;

    assert_eq!(
        Query::default().effective_tool_choice(&cfg),
        ToolChoice::Required
    );
}

#[test]
fn test_tool_use_of_locked_off_tool_errors() {
    // A locked-off tool cannot be forced: the refusal names the lock rather
    // than claiming the tool does not exist.
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
            tool_use: Some(Some("network".to_owned())),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap_err()
    .to_string();

    assert_eq!(
        err,
        "cannot enable `network`: this tool is configured as locked-off"
    );
}

#[test]
fn test_tool_use_of_unknown_tool_errors() {
    let err = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_use: Some(Some("no_such_tool".to_owned())),
            ..Default::default()
        },
        None,
        make_partial_with_tools(),
        None,
    )
    .unwrap_err()
    .to_string();

    // Pins the message this PR introduced: the old one said "any enabled
    // tools", which a `contains("no_such_tool")` check could not tell apart.
    assert_eq!(
        err,
        "tool choice 'no_such_tool' does not match any known tools"
    );
}

#[test]
fn test_tool_use_does_not_persist_into_partial_config() {
    // `-u` binds one turn. Mirrors `no_title_does_not_persist_into_partial_config`:
    // anything this flag writes into the partial would reach the conversation
    // through `turn_config_delta` and force the tool on every later
    // query.
    let base = make_partial_with_tools();

    let with_flag = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_use: Some(Some("explicitly_disabled_tool".to_owned())),
            ..Default::default()
        },
        None,
        base.clone(),
        None,
    )
    .unwrap();
    let without_flag =
        IntoPartialAppConfig::apply_cli_config(&Query::default(), None, base, None).unwrap();

    assert_eq!(with_flag.assistant.tool_choice, None);
    assert_eq!(
        effective(&with_flag, "explicitly_disabled_tool").state,
        effective(&without_flag, "explicitly_disabled_tool").state,
    );
}

#[test]
fn test_no_tool_use_does_not_persist_into_partial_config() {
    // `-U` is the single-turn counterpart of `-T`, so it must not write
    // `tool_choice = none` into the conversation either.
    let partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            no_tool_use: true,
            ..Default::default()
        },
        None,
        make_partial_with_tools(),
        None,
    )
    .unwrap();

    assert_eq!(partial.assistant.tool_choice, None);

    assert_eq!(
        Query {
            no_tool_use: true,
            ..Default::default()
        }
        .effective_tool_choice(&AppConfig::new_test()),
        ToolChoice::None,
        "the flag still takes effect for this turn"
    );
}

#[test]
fn test_tool_directive_does_persist_into_partial_config() {
    // The deliberate asymmetry: `-t` is the sticky flag. Enabling a tool has to
    // reach the config, because that is the only way to make it stick, and
    // `-T` is the documented undo.
    let partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_directives: directives(vec![ToolDirective::Enable(
                "explicitly_disabled_tool".into(),
            )]),
            ..Default::default()
        },
        None,
        make_partial_with_tools(),
        None,
    )
    .unwrap();

    assert!(effective(&partial, "explicitly_disabled_tool").state);
}

#[test]
fn test_tool_use_forces_a_tool_a_directive_disabled() {
    // `jp q -T my_tool -u my_tool`: the two no longer contradict. `-T` disables
    // the tool durably, `-u` forces it for this turn only, and both hold.
    let query = Query {
        tool_directives: directives(vec![ToolDirective::Disable(
            "implicitly_enabled_tool".into(),
        )]),
        tool_use: Some(Some("implicitly_enabled_tool".to_owned())),
        ..Default::default()
    };

    let partial =
        IntoPartialAppConfig::apply_cli_config(&query, None, make_partial_with_tools(), None)
            .unwrap();

    assert!(
        !effective(&partial, "implicitly_enabled_tool").state,
        "`-T` still persists the disable"
    );
    assert_eq!(
        query.effective_tool_choice(&AppConfig::new_test()),
        ToolChoice::Function("implicitly_enabled_tool".into()),
        "`-u` still forces the tool for this turn"
    );
}

#[test]
fn test_builtin_config_merges_under_user_config() {
    // A one-field user override keeps the built-in source, enable policy, and parameters.
    let mut partial = make_partial_with_tools();
    partial
        .conversation
        .tools
        .tools
        .insert("describe_tools".into(), PartialToolConfig {
            result: Some(ResultMode::Ask),
            ..Default::default()
        });

    let partial =
        IntoPartialAppConfig::apply_cli_config(&Query::default(), None, partial, None).unwrap();

    let tool = &partial.conversation.tools.tools["describe_tools"];
    assert_eq!(tool.result, Some(ResultMode::Ask), "user field wins");
    assert_eq!(
        tool.source,
        Some(ToolSource::Builtin { tool: None }),
        "builtin source survives a partial user override"
    );
    assert!(
        effective(&partial, "describe_tools").is_locked(),
        "builtin enable policy survives a partial user override"
    );
    assert!(
        tool.parameters.contains_key("tools"),
        "builtin parameter schema survives a partial user override"
    );
}

/// Naming a tool after a built-in shadows it rather than replacing it: the
/// user's `source` and `command` win, and every field they leave unset comes
/// from the built-in's block.
/// That includes `run = "allow"`, so a shadowing local command runs without a
/// confirmation prompt unless the user sets `run` themselves.
/// RFD 083 calls this out as user-owned consequences, not a blocked
/// configuration; this test pins it so the behavior cannot drift silently.
#[test]
fn test_builtin_config_merges_under_a_shadowing_local_tool() {
    let mut partial = make_partial_with_tools();
    partial
        .conversation
        .tools
        .tools
        .insert("describe_tools".into(), PartialToolConfig {
            source: Some(ToolSource::Local { tool: None }),
            command: Some(PartialCommandConfigOrString::String(
                "my-describe-tools".to_owned(),
            )),
            ..Default::default()
        });

    let partial =
        IntoPartialAppConfig::apply_cli_config(&Query::default(), None, partial, None).unwrap();

    let tool = &partial.conversation.tools.tools["describe_tools"];
    assert_eq!(
        tool.source,
        Some(ToolSource::Local { tool: None }),
        "the user's source wins, so their executor runs"
    );
    assert_eq!(
        tool.run,
        Some(RunMode::Allow),
        "an unset `run` comes from the built-in, not from the global default"
    );
    assert!(
        tool.parameters.contains_key("tools"),
        "an unset `parameters` keeps the built-in's schema"
    );
    assert!(
        effective(&partial, "describe_tools").is_locked(),
        "an unset `enable` keeps the built-in's locked-on policy"
    );
}

#[test]
fn test_builtin_config_preserves_tool_order() {
    // Tool order is the order tools are presented to the provider, so merging
    // a builtin block must not move an existing entry to the end.
    let mut partial = PartialAppConfig::default();
    *partial.conversation.tools.tools = IndexMap::from_iter([
        ("describe_tools".into(), PartialToolConfig {
            result: Some(ResultMode::Ask),
            ..Default::default()
        }),
        ("zzz_last".into(), PartialToolConfig::default()),
    ]);

    let partial =
        IntoPartialAppConfig::apply_cli_config(&Query::default(), None, partial, None).unwrap();

    let names: Vec<&str> = partial
        .conversation
        .tools
        .tools
        .keys()
        .map(String::as_str)
        .filter(|name| matches!(*name, "describe_tools" | "zzz_last"))
        .collect();
    assert_eq!(names, vec!["describe_tools", "zzz_last"]);
}

/// The seam between merging built-in config under user config and validating
/// `-u`.
/// A table override sets only `state`, so `allow_toggle = never` survives from
/// the built-in block and the tool ends up locked *off* rather than merely
/// disabled.
/// Forcing it is then refused, which is the documented meaning of the table
/// form (RFD 083) but only became reachable once both halves landed.
#[test]
fn test_table_disabled_builtin_cannot_be_forced() {
    let mut partial = make_partial_with_tools();
    // `[conversation.tools.describe_tools] enable = { state = false }`
    partial
        .conversation
        .tools
        .tools
        .insert("describe_tools".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig {
                state: Some(false),
                allow_toggle: None,
            }),
            ..Default::default()
        });

    let err = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tool_use: Some(Some("describe_tools".to_owned())),
            ..Default::default()
        },
        None,
        partial,
        None,
    )
    .unwrap_err()
    .to_string();

    assert_eq!(
        err,
        "cannot enable `describe_tools`: this tool is configured as locked-off"
    );
}

/// The escape hatch for the above: the bool shorthand writes `allow_toggle =
/// any` explicitly, so it overrides the built-in's policy as well as its state
/// and the tool stays forceable.
#[test]
fn test_bool_disabled_builtin_can_still_be_forced() {
    let mut partial = make_partial_with_tools();
    // `[conversation.tools.describe_tools] enable = false`
    partial
        .conversation
        .tools
        .tools
        .insert("describe_tools".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig::OFF),
            ..Default::default()
        });

    let query = Query {
        tool_use: Some(Some("describe_tools".to_owned())),
        ..Default::default()
    };

    let partial = IntoPartialAppConfig::apply_cli_config(&query, None, partial, None)
        .expect("the bool shorthand leaves the tool forceable");

    assert_eq!(
        effective(&partial, "describe_tools").allow_toggle,
        AllowToggle::Always,
        "the shorthand overrides the built-in's toggle policy, not just its state"
    );
    assert_eq!(
        query.effective_tool_choice(&AppConfig::new_test()),
        ToolChoice::Function("describe_tools".into())
    );
}

#[test]
fn resolve_config_keeps_a_cleared_conversation_field_across_invocations() {
    let tmp = Utf8TempDir::new().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    workspace.load_conversation_index();
    let mut base = AppConfig::new_test();
    base.assistant.name = Some("Bot".to_owned());
    let base = Arc::new(base);
    let conversation_id = make_id(4001);
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base),
    );
    let id = conversation_id.to_string();
    let cli = Cli::try_parse_from([
        "jp",
        "query",
        "--id",
        &id,
        "--cfg",
        "assistant.name:=null",
        "hello",
    ])
    .unwrap();

    let Resolved {
        config: first,
        mut handles,
        ..
    } = resolve_config(
        cli.command,
        || Ok(base.to_partial()),
        &cli.globals.config,
        &mut workspace,
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(first.assistant.name, None);
    {
        let lock = workspace.test_lock(handles.remove(0));
        let delta = turn_config_delta(&first, &lock).unwrap().unwrap();
        assert_eq!(delta.unsets, ["assistant.name"]);
        lock.as_mut()
            .update_events(|events| events.add_config_delta(delta));
        assert_eq!(lock.events().config().unwrap().assistant.name, None);
    }

    let cli = Cli::try_parse_from(["jp", "query", "--id", &id, "hello again"]).unwrap();
    let Resolved {
        config: second,
        mut handles,
        ..
    } = resolve_config(
        cli.command,
        || Ok(base.to_partial()),
        &cli.globals.config,
        &mut workspace,
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(second.assistant.name, None);
    let lock = workspace.test_lock(handles.remove(0));
    assert_eq!(turn_config_delta(&second, &lock).unwrap(), None);
    assert_eq!(lock.events().config_deltas().count(), 1);
}

fn section(tag: &str, position: i32) -> PartialSectionConfig {
    PartialSectionConfig {
        content: Some(format!("{tag} content")),
        tag: Some(tag.to_owned()),
        title: None,
        position: Some(position),
    }
}

/// Run two queries without CLI overrides against a conversation initialized
/// from `base`, and return the config delta produced by the second query.
///
/// The first query may record a delta for the built-in tools injected during
/// query config resolution.
fn second_plain_query_delta(base: &PartialAppConfig) -> Option<ApplyDelta> {
    let tmp = Utf8TempDir::new().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    workspace.load_conversation_index();
    let conversation_id = make_id(4002);
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::new(build(base.clone()).unwrap()),
    );
    let id = conversation_id.to_string();

    let mut delta = None;
    for _ in 0..2 {
        let cli = Cli::try_parse_from(["jp", "query", "--id", &id, "hello"]).unwrap();
        let Resolved {
            config,
            mut handles,
            ..
        } = resolve_config(
            cli.command,
            || Ok(base.clone()),
            &cli.globals.config,
            &mut workspace,
            None,
            None,
            false,
        )
        .unwrap();

        let lock = workspace.test_lock(handles.remove(0));
        delta = turn_config_delta(&config, &lock).unwrap();
        if let Some(delta) = delta.clone() {
            lock.as_mut()
                .update_events(|events| events.add_config_delta(delta));
        }
    }

    delta
}

#[test]
fn plain_query_records_no_delta_for_unsorted_prompt_sections() {
    let mut base = AppConfig::new_test().to_partial();
    base.assistant.system_prompt_sections =
        MergeableVec::Vec(vec![section("late", 10), section("early", -10)]);

    assert_eq!(second_plain_query_delta(&base), None);
}

#[test]
fn plain_query_records_no_delta_for_repeated_prompt_sections() {
    let mut base = AppConfig::new_test().to_partial();
    base.assistant.system_prompt_sections =
        MergeableVec::Vec(vec![section("a", 0), section("b", 0), section("a", 0)]);

    assert_eq!(second_plain_query_delta(&base), None);
}

/// A built-in registered as locked-on must survive config resolution as an
/// enabled tool.
/// Before RFD 081 the enable predicate matched only one variant, so this
/// resolved to disabled and the tool was silently dropped.
#[test]
fn test_describe_tools_is_enabled_in_resolved_config() {
    let partial = IntoPartialAppConfig::apply_cli_config(
        &Query::default(),
        None,
        AppConfig::new_test().to_partial(),
        None,
    )
    .unwrap();

    let cfg = build(partial).unwrap();
    let tool = cfg
        .conversation
        .tools
        .get("describe_tools")
        .expect("builtin is present in the resolved config");

    assert!(tool.is_enabled(), "describe_tools must be enabled");
    assert!(matches!(tool.source(), ToolSource::Builtin { .. }));
}

/// The other half of the same guard: `tool_definitions` is the filter that
/// dropped the tool, so assert it survives all the way to the list handed to
/// the provider.
/// A registered executor is unreachable if the definition never ships.
#[tokio::test]
async fn test_describe_tools_reaches_the_llm_tool_list() {
    let partial = IntoPartialAppConfig::apply_cli_config(
        &Query::default(),
        None,
        AppConfig::new_test().to_partial(),
        None,
    )
    .unwrap();

    let cfg = build(partial).unwrap();
    let client = jp_mcp::Client::new(IndexMap::new());
    let defs = tool_definitions(cfg.conversation.tools.iter(), &client, None)
        .await
        .unwrap();

    assert!(
        defs.iter().any(|d| d.name == "describe_tools"),
        "describe_tools must be sent to the provider, got: {:?}",
        defs.iter().map(|d| &d.name).collect::<Vec<_>>()
    );
}

#[test]
fn query_model_override_is_persisted_as_config_delta() {
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(1000);

    let mut workspace = Workspace::in_memory("/tmp/test");
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

    let delta = turn_config_delta(&runtime_config, &lock)
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
fn tier_flag_is_persisted_as_config_delta() {
    // Each tier is served from separate provider capacity, so a tier that
    // reverted after one turn would throw the prompt cache away twice: once on
    // the way in and once on the way back out. `--tier` is durable for that
    // reason, unlike the invocation-scoped `-u`/`-U`.
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(1100);

    let mut workspace = Workspace::in_memory("/tmp/test");
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );

    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    let query = Query {
        service_tier: Some(ServiceTier::Flex),
        ..Default::default()
    };

    let partial = query
        .apply_cli_config(None, base_config.to_partial(), None)
        .unwrap();
    let runtime_config = build(partial).unwrap();

    let delta = turn_config_delta(&runtime_config, &lock)
        .unwrap()
        .expect("expected --tier to produce a config delta");

    lock.as_mut()
        .update_events(|events| events.add_config_delta(delta));

    let merged = lock.events().config().unwrap();
    assert_eq!(
        merged.assistant.model.parameters.service_tier,
        Some(ServiceTier::Flex),
        "the tier must still apply on the next turn, without the flag"
    );
}

/// `--no-tier` is `--tier off`, and it takes a tier off a conversation that
/// already carries one.
///
/// `off` rather than an absent field: a partial's `None` means "no opinion", so
/// it would leave the earlier layer's tier standing and the flag would appear
/// to do nothing on the next turn.
#[test]
fn no_tier_flag_replaces_a_persisted_tier_with_off() {
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(1200);

    let mut workspace = Workspace::in_memory("/tmp/test");
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );

    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    // Put a tier on the conversation, the way an earlier `--tier priority` did.
    let mut seeded = PartialAppConfig::empty();
    seeded.assistant.model.parameters.service_tier = Some(ServiceTier::Priority);
    lock.as_mut()
        .update_events(|events| events.add_config_delta(ApplyDelta::new(Utc::now(), seeded)));

    let stored = lock.events().config().unwrap();
    assert_eq!(
        stored.assistant.model.parameters.service_tier,
        Some(ServiceTier::Priority),
        "the fixture must start with a tier to turn off"
    );

    let query = Query {
        no_service_tier: true,
        ..Default::default()
    };
    let partial = query
        .apply_cli_config(None, stored.to_partial(), None)
        .unwrap();
    let runtime_config = build(partial).unwrap();

    assert_eq!(
        runtime_config.assistant.model.parameters.service_tier,
        Some(ServiceTier::Off),
        "--no-tier must resolve this query to `off`"
    );

    let delta = turn_config_delta(&runtime_config, &lock)
        .unwrap()
        .expect("turning the tier off must produce a config delta");

    lock.as_mut()
        .update_events(|events| events.add_config_delta(delta));

    assert_eq!(
        lock.events()
            .config()
            .unwrap()
            .assistant
            .model
            .parameters
            .service_tier,
        Some(ServiceTier::Off),
        "`off` must still apply on the next turn, without the flag"
    );
}

/// `off` survives being layered under a config file that asks for a tier.
///
/// The conversation layer is gap-filled from the file layer rather than merged
/// onto it, so an absent tier would be refilled with the file's and the flag
/// would silently stop working after one turn.
#[test]
fn off_is_not_refilled_from_the_file_layer() {
    use jp_config::FillDefaults as _;

    let mut pipeline_base = PartialAppConfig::default();
    pipeline_base.assistant.model.parameters.service_tier = Some(ServiceTier::Flex);

    let mut conversation = PartialAppConfig::empty();
    conversation.assistant.model.parameters.service_tier = Some(ServiceTier::Off);

    let filled = conversation.fill_from(pipeline_base);

    assert_eq!(
        filled.assistant.model.parameters.service_tier,
        Some(ServiceTier::Off)
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

    let mut workspace = Workspace::in_memory("/tmp/test");
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
        reasoning: Some(ReasoningMode::Strip.into()),
        ..Default::default()
    }]);
    let runtime_config = build(partial).unwrap();

    let delta = turn_config_delta(&runtime_config, &lock)
        .unwrap()
        .expect("cfg-sourced compaction config should produce a delta");

    assert!(
        !delta.delta.conversation.compaction.rules.is_empty(),
        "compaction config from the config layers must persist as a conversation delta",
    );
}

/// The `config_delta` events of a stream's serialized `events.json`.
fn serialized_config_deltas(events: &ConversationStream) -> Vec<Value> {
    let (_base, serialized) = events.clone().to_parts().unwrap();
    serialized
        .into_iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("config_delta"))
        .collect()
}

#[test]
fn cfg_reset_none_appends_reset_then_post_apply() {
    // `jp q --cfg=NONE --cfg=<post>` on a continuing conversation ([RFD 038]):
    // the stream records `[Reset, Apply(post)]`, where `post` restores the
    // required fields on top of program defaults.
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(3000);

    let mut workspace = Workspace::in_memory("/tmp/test");
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );
    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    let post = config_with_model(ProviderId::Openai, "fresh-model").to_partial();
    let reset = ConfigResetEvents {
        reset: ConfigReset::Defaults,
        post: Box::new(post),
    };

    lock.as_mut()
        .update_events(|events| persist_config_reset(events, reset, DateTime::<Utc>::UNIX_EPOCH));

    let events = lock.events().clone();

    // The stream resolves to the post-reset state, not the pre-reset base.
    let merged = events.config().unwrap();
    let model_id = merged.assistant.model.id.resolved();
    assert_eq!(model_id.provider, ProviderId::Openai);
    assert_eq!(model_id.name.as_ref(), "fresh-model");

    // Wire shape: a `Reset` marker followed by a plain `Apply` (no `op`).
    let deltas = serialized_config_deltas(&events);
    assert_eq!(deltas.len(), 2, "expected [Reset, Apply], got {deltas:?}");
    assert_eq!(deltas[0].get("op").and_then(Value::as_str), Some("reset"));
    assert!(
        deltas[1].get("op").is_none(),
        "`Apply` writes no `op` field"
    );
}

#[test]
fn cfg_reset_workspace_appends_reset_then_workspace_apply() {
    // `jp q --cfg=WORKSPACE` on a continuing conversation ([RFD 038]): the
    // stream records `[Reset, Apply(workspace)]`, re-adopting the workspace's
    // resolved config as of this invocation. No further `Apply` is written
    // when nothing is layered on top.
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(3001);

    let mut workspace = Workspace::in_memory("/tmp/test");
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );
    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    let workspace_partial = config_with_model(ProviderId::Openai, "ws-model").to_partial();
    let reset = ConfigResetEvents {
        reset: ConfigReset::Workspace(Box::new(workspace_partial)),
        post: Box::new(PartialAppConfig::default()),
    };

    lock.as_mut()
        .update_events(|events| persist_config_reset(events, reset, DateTime::<Utc>::UNIX_EPOCH));

    let events = lock.events().clone();

    let merged = events.config().unwrap();
    let model_id = merged.assistant.model.id.resolved();
    assert_eq!(model_id.provider, ProviderId::Openai);
    assert_eq!(model_id.name.as_ref(), "ws-model");

    let deltas = serialized_config_deltas(&events);
    assert_eq!(
        deltas.len(),
        2,
        "empty post-reset state must not write a third event: {deltas:?}"
    );
    assert_eq!(deltas[0].get("op").and_then(Value::as_str), Some("reset"));
    assert!(deltas[1].get("op").is_none());
}

#[test]
fn cfg_reset_none_without_post_leaves_unresolvable_config() {
    // A bare `jp q --cfg=NONE` on a continuing conversation records only the
    // `Reset`. Program defaults lack required fields (`assistant.model.id`),
    // so resolving the stream's config fails until a later `Apply` restores
    // them — the intended escape-hatch semantics from [RFD 038].
    let base_config = Arc::new(config_with_model(ProviderId::Anthropic, "base-model"));
    let conversation_id = make_id(3002);

    let mut workspace = Workspace::in_memory("/tmp/test");
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );
    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    let reset = ConfigResetEvents {
        reset: ConfigReset::Defaults,
        post: Box::new(PartialAppConfig::default()),
    };

    lock.as_mut()
        .update_events(|events| persist_config_reset(events, reset, DateTime::<Utc>::UNIX_EPOCH));

    let events = lock.events().clone();

    let deltas = serialized_config_deltas(&events);
    assert_eq!(deltas.len(), 1, "expected only the Reset: {deltas:?}");
    assert_eq!(deltas[0].get("op").and_then(Value::as_str), Some("reset"));

    assert!(
        events.config().is_err(),
        "program defaults alone must fail validation"
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
    base.config_load_paths = Some(vec![RelativePathBuf::from(".jp/config")].into());
    base.providers.llm.aliases.insert(
        "gpt".to_owned(),
        ModelIdConfig {
            provider: ProviderId::Openai,
            name: "gpt-model".parse().unwrap(),
        }
        .to_partial()
        .into(),
    );

    let mut workspace = Workspace::in_memory(root);

    let query1 = Query {
        new_conversation: true,
        query: inline_query("is this thing on?"),
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
    run_mock_turn(&cfg1, &lock1, "is this thing on?", "Yes, loud and clear.").await;
    drop(lock1);

    let handle2 = workspace.acquire_conversation(&conversation_id).unwrap();
    let query2 = Query {
        model: Some("gpt".to_owned()),
        query: inline_query("are you there?"),
        ..Default::default()
    };
    let cfg2 = build_query_config(&workspace, base.clone(), &[], &query2, Some(&handle2));
    let lock2 = workspace.test_lock(handle2);
    let delta = turn_config_delta(&cfg2, &lock2)
        .unwrap()
        .expect("expected model override to persist");
    lock2
        .as_mut()
        .update_events(|events| events.add_config_delta(delta));
    run_mock_turn(&cfg2, &lock2, "are you there?", "Yes.").await;
    drop(lock2);

    let handle3 = workspace.acquire_conversation(&conversation_id).unwrap();
    let query3 = Query {
        query: inline_query("plain query"),
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

/// A conversation keeps the default plugin policy it was created under.
///
/// Its stored configuration states `plugins.command.'*'.run`, and the
/// conversation layer sits above the config files, so a default the files
/// change afterwards reaches new conversations and not this one.
/// `--cfg` sits above the conversation layer, and does reach it.
#[tokio::test]
async fn a_conversation_keeps_the_default_plugin_policy_it_was_created_under() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());

    // Created while the config files leave the default at `ask`.
    let asking = AppConfig::new_test().to_partial();
    let first = Query {
        new_conversation: true,
        query: inline_query("hello"),
        ..Default::default()
    };
    let created = build_query_config(&workspace, asking.clone(), &[], &first, None);
    assert_eq!(created.plugins.command.defaults.run, RunPolicy::Ask);

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), Arc::new(created.clone()), None)
        .unwrap();
    let conversation_id = lock.id();
    run_mock_turn(&created, &lock, "hello", "Hi.").await;
    drop(lock);

    // From here on the config files deny every plugin that is not named.
    let mut denying = asking.clone();
    denying.plugins.command.defaults.run = Some(RunPolicy::Deny);

    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let next = Query {
        query: inline_query("again"),
        ..Default::default()
    };

    let resumed = build_query_config(&workspace, denying.clone(), &[], &next, Some(&handle));
    assert_eq!(
        resumed.plugins.command.defaults.run,
        RunPolicy::Ask,
        "the conversation's stored default wins over the config files"
    );

    let fresh = build_query_config(&workspace, denying, &[], &first, None);
    assert_eq!(
        fresh.plugins.command.defaults.run,
        RunPolicy::Deny,
        "a new conversation takes the default the config files set"
    );

    let deny = "plugins.command.*.run=deny"
        .parse::<KeyValueOrPath>()
        .unwrap();
    let overridden = build_query_config(&workspace, asking, &[deny], &next, Some(&handle));
    assert_eq!(
        overridden.plugins.command.defaults.run,
        RunPolicy::Deny,
        "`--cfg` wins over the conversation's stored default"
    );
}

/// A partial naming `model` as the turn's model.
fn partial_with_model(model: PartialModelIdOrAliasConfig) -> PartialAppConfig {
    let mut partial = PartialAppConfig::default();
    partial.assistant.model.id = model;
    partial
}

fn model_id(id: &str) -> PartialModelIdOrAliasConfig {
    PartialModelIdOrAliasConfig::Id(id.parse().expect("a valid model id"))
}

fn alias(name: &str) -> PartialModelIdOrAliasConfig {
    PartialModelIdOrAliasConfig::Alias(name.to_owned())
}

/// The chain lands on the provider of the model serving the turn, and nowhere
/// else.
#[test]
fn test_auth_writes_the_chain_to_the_turns_provider() {
    let mut partial = partial_with_model(model_id("openai/gpt-5.6-luna"));
    let auth = vec![AuthEntry::Subscription(Some("personal".to_owned()))];

    apply_auth(&mut partial, &auth, None).unwrap();

    assert_eq!(partial.providers.llm.openai.auth.as_ref(), Some(&auth));
    assert_eq!(partial.providers.llm.anthropic.auth, None);
    assert_eq!(partial.providers.llm.cerebras.auth, None);
}

/// A whole chain is written, not only its first entry.
#[test]
fn test_auth_writes_every_entry_in_order() {
    let mut partial = partial_with_model(model_id("anthropic/claude-haiku-4-5"));
    let auth = vec![
        AuthEntry::Named("personal".to_owned()),
        AuthEntry::ApiKey(None),
    ];

    apply_auth(&mut partial, &auth, None).unwrap();

    assert_eq!(partial.providers.llm.anthropic.auth.as_ref(), Some(&auth));
}

/// An alias is followed to the provider it stands for.
#[test]
fn test_auth_follows_an_alias_to_its_provider() {
    let mut partial = partial_with_model(alias("luna"));
    partial
        .providers
        .llm
        .aliases
        .insert("luna".to_owned(), model_id("openai/gpt-5.6-luna"));

    apply_auth(&mut partial, &[AuthEntry::ApiKey(None)], None).unwrap();

    assert_eq!(
        partial.providers.llm.openai.auth,
        Some(vec![AuthEntry::ApiKey(None)])
    );
}

/// An alias naming another alias resolves through to the model id.
#[test]
fn test_auth_follows_a_chain_of_aliases() {
    let mut partial = partial_with_model(alias("fast"));
    partial
        .providers
        .llm
        .aliases
        .insert("fast".to_owned(), alias("luna"));
    partial
        .providers
        .llm
        .aliases
        .insert("luna".to_owned(), model_id("openai/gpt-5.6-luna"));

    apply_auth(&mut partial, &[AuthEntry::ApiKey(None)], None).unwrap();

    assert!(partial.providers.llm.openai.auth.is_some());
}

/// An alias pointing at itself reports rather than recursing forever.
#[test]
fn test_auth_gives_up_on_an_alias_cycle() {
    let mut partial = partial_with_model(alias("a"));
    partial
        .providers
        .llm
        .aliases
        .insert("a".to_owned(), alias("b"));
    partial
        .providers
        .llm
        .aliases
        .insert("b".to_owned(), alias("a"));

    let error = apply_auth(&mut partial, &[AuthEntry::ApiKey(None)], None).unwrap_err();

    assert!(
        error.to_string().contains("--auth needs to know"),
        "{error}"
    );
}

/// The provider comes from the config layers when the CLI names no model.
#[test]
fn test_auth_reads_the_provider_from_the_merged_config() {
    let mut partial = PartialAppConfig::default();
    let merged = partial_with_model(model_id("cerebras/gpt-oss-120b"));

    apply_auth(&mut partial, &[AuthEntry::ApiKey(None)], Some(&merged)).unwrap();

    assert!(partial.providers.llm.cerebras.auth.is_some());
}

/// The CLI's own model wins over the one the config layers settled on.
#[test]
fn test_auth_prefers_the_models_named_on_the_command_line() {
    let mut partial = partial_with_model(model_id("openai/gpt-5.6-luna"));
    let merged = partial_with_model(model_id("anthropic/claude-haiku-4-5"));

    apply_auth(&mut partial, &[AuthEntry::ApiKey(None)], Some(&merged)).unwrap();

    assert!(partial.providers.llm.openai.auth.is_some());
    assert_eq!(partial.providers.llm.anthropic.auth, None);
}

/// An unresolvable provider names the `--cfg` form that would work, rather than
/// writing the chain somewhere it would do nothing.
#[test]
fn test_auth_without_a_provider_names_the_alternative() {
    let mut partial = PartialAppConfig::default();
    let auth = vec![
        AuthEntry::Subscription(Some("personal".to_owned())),
        AuthEntry::ApiKey(None),
    ];

    let error = apply_auth(&mut partial, &auth, None).unwrap_err();
    let message = error.to_string();

    assert!(message.contains("--model <provider>/<name>"), "{message}");
    assert!(
        message.contains("providers.llm.<provider>.auth=subscription:personal,api_key"),
        "{message}"
    );
}

/// A provider reached over a local socket has no credential to choose.
#[test]
fn test_auth_is_refused_for_a_provider_without_credentials() {
    for model in ["llamacpp/qwen3", "ollama/qwen3"] {
        let mut partial = partial_with_model(model_id(model));

        let error = apply_auth(&mut partial, &[AuthEntry::ApiKey(None)], None).unwrap_err();

        assert!(
            error.to_string().contains("needs no credential"),
            "{model}: {error}"
        );
    }
}

/// Without the flag, nothing is written and no provider is looked for.
#[test]
fn test_auth_is_a_no_op_when_the_flag_is_absent() {
    let mut partial = PartialAppConfig::default();

    apply_auth(&mut partial, &[], None).unwrap();

    assert_eq!(partial.providers.llm.openai.auth, None);
    assert_eq!(partial.providers.llm.anthropic.auth, None);
}

#[test]
fn test_attach_rejects_a_url_its_handler_cannot_resolve() {
    let diff = Url::parse("gh:pull/1/diff").unwrap();
    let mut partial = PartialAppConfig::default();
    partial.conversation.attachments.push(diff.clone().into());

    let error = apply_attachments(
        &mut partial,
        &[AttachmentUrlOrPath::Url(
            Url::parse("cmd://printenv?arg=TERM&env=TERM=xterm").unwrap(),
        )],
        None,
    )
    .unwrap_err();

    assert_eq!(
        crate::error::error_chain(&error),
        "Attachment error for cmd://printenv?arg=TERM&env=TERM=xterm: invalid `env` value \
         `TERM=xterm`: expected the name of an environment variable, such as `TERM`"
    );
    assert_eq!(partial.conversation.attachments.to_vec(), vec![
        PartialAttachmentConfig::Url(diff)
    ]);
}

#[test]
fn test_attach_skips_a_url_the_conversation_already_attaches() {
    let diff = Url::parse("gh:pull/1/diff").unwrap();
    let reviews = Url::parse("gh:pull/1/reviews").unwrap();
    let mut partial = PartialAppConfig::default();
    partial.conversation.attachments.push(diff.clone().into());

    apply_attachments(
        &mut partial,
        &[
            AttachmentUrlOrPath::Url(diff.clone()),
            AttachmentUrlOrPath::Url(reviews.clone()),
        ],
        None,
    )
    .unwrap();

    assert_eq!(partial.conversation.attachments.to_vec(), vec![
        PartialAttachmentConfig::Url(diff),
        PartialAttachmentConfig::Url(reviews),
    ]);
}
