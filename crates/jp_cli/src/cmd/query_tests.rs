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

#[test]
#[expect(clippy::too_many_lines)]
fn test_query_tools_and_no_tools() {
    // Create a partial configuration with a few tools.
    let mut partial = make_partial_with_tools();

    // Keep all tools as-is.
    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            no_tools: vec![],
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
            no_tools: vec![Some("implicitly_enabled_tool".into())],
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
            tools: vec![Some("explicitly_disabled_tool".into())],
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

    // Enable all tools — explicit tools should stay explicit.
    partial = IntoPartialAppConfig::apply_cli_config(
        &Query {
            tools: vec![None],
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
            no_tools: vec![None],
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
            tools: vec![
                Some("explicitly_disabled_tool".into()),
                Some("explicitly_enabled_tool".into()),
            ],
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
            tools: vec![Some("explicit_tool".into())],
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
            tools: vec![None, Some("explicit_tool".into())],
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
            tools: vec![None],
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
