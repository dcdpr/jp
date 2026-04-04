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
