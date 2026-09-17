use serde_json::from_str;

use super::*;
use crate::assignment::KvAssignment;

#[test]
fn test_link_style_deserialization() {
    assert_eq!(from_str::<LinkStyle>("false").unwrap(), LinkStyle::Off);
    assert_eq!(from_str::<LinkStyle>("true").unwrap(), LinkStyle::Full);
    assert_eq!(from_str::<LinkStyle>("\"off\"").unwrap(), LinkStyle::Off);
    assert_eq!(from_str::<LinkStyle>("\"full\"").unwrap(), LinkStyle::Full);
    assert_eq!(from_str::<LinkStyle>("\"osc8\"").unwrap(), LinkStyle::Osc8);
}

#[test]
fn test_inline_results_deserialization() {
    assert_eq!(
        from_str::<InlineResults>("false").unwrap(),
        InlineResults::Off
    );
    assert_eq!(
        from_str::<InlineResults>("true").unwrap(),
        InlineResults::Full
    );
    assert_eq!(
        from_str::<InlineResults>("\"off\"").unwrap(),
        InlineResults::Off
    );
    assert_eq!(
        from_str::<InlineResults>("\"full\"").unwrap(),
        InlineResults::Full
    );
    assert_eq!(
        from_str::<InlineResults>("10").unwrap(),
        InlineResults::Truncate(TruncateLines { lines: 10 })
    );
    assert_eq!(
        from_str::<InlineResults>("\"25\"").unwrap(),
        InlineResults::Truncate(TruncateLines { lines: 25 })
    );
    assert_eq!(
        from_str::<InlineResults>(r#"{"truncate": {"lines": 5}}"#).unwrap(),
        InlineResults::Truncate(TruncateLines { lines: 5 })
    );
}

#[test]
fn assign_joins_reasoning_accepts_a_bool() {
    let mut partial = PartialDisplayStyleConfig::default();
    let kv = KvAssignment::try_from_cli("joins_reasoning", "false").unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(partial.joins_reasoning, Some(false));
}

/// An unset `joins_reasoning` takes the value from the `*` defaults block, so
/// one entry there covers every tool.
#[test]
fn joins_reasoning_fills_from_the_defaults_block() {
    let filled = PartialDisplayStyleConfig::default().fill_from(PartialDisplayStyleConfig {
        joins_reasoning: Some(false),
        ..Default::default()
    });

    assert_eq!(filled.joins_reasoning, Some(false));
}

/// A tool that states its own value keeps it: the defaults block fills gaps
/// rather than overriding.
#[test]
fn an_explicit_joins_reasoning_survives_the_defaults_block() {
    let filled = PartialDisplayStyleConfig {
        joins_reasoning: Some(true),
        ..Default::default()
    }
    .fill_from(PartialDisplayStyleConfig {
        joins_reasoning: Some(false),
        ..Default::default()
    });

    assert_eq!(filled.joins_reasoning, Some(true));
}

#[test]
fn test_assign_error_inline_results_only_sets_overlay_field() {
    let mut partial = PartialDisplayStyleConfig::default();
    let kv = KvAssignment::try_from_cli("error.inline_results", "full").unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(partial.error.inline_results, Some(InlineResults::Full));
    assert_eq!(partial.error.results_file_link, None);
    assert_eq!(partial.inline_results, None);
    assert_eq!(partial.results_file_link, None);
}

#[test]
fn test_assign_error_block_via_json() {
    let mut partial = PartialDisplayStyleConfig::default();
    let kv = KvAssignment::try_from_cli(
        "error:",
        r#"{"inline_results":"off","results_file_link":"osc8"}"#,
    )
    .unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(partial.error.inline_results, Some(InlineResults::Off));
    assert_eq!(partial.error.results_file_link, Some(LinkStyle::Osc8));
}

#[test]
fn test_error_overlay_falls_back_per_field() {
    let style = DisplayStyleConfig {
        hidden: false,
        joins_reasoning: true,
        parameters: ParametersStyle::Json,
        inline_results: InlineResults::Truncate(TruncateLines { lines: 5 }),
        results_file_link: LinkStyle::Full,
        print_stderr: true,
        error: ErrorStyleConfig {
            inline_results: Some(InlineResults::Full),
            results_file_link: None,
        },
    };

    // Field set on the overlay: overlay wins.
    let il = style
        .error
        .inline_results
        .as_ref()
        .unwrap_or(&style.inline_results);
    assert_eq!(*il, InlineResults::Full);

    // Field unset on the overlay: inherits from the top-level field.
    let rfl = style
        .error
        .results_file_link
        .as_ref()
        .unwrap_or(&style.results_file_link);
    assert_eq!(*rfl, LinkStyle::Full);
}
