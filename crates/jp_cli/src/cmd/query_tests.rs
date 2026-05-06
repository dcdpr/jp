use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use jp_config::conversation::tool::{Enable, PartialToolConfig};

use super::*;

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
