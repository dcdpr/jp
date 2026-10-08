use indoc::indoc;
use schematic::PartialConfig as _;

use super::*;
use crate::{AppConfig, PartialAppConfig, style::window_rows::RowCount};

#[test]
fn the_window_is_off_by_default_and_shows_a_timer() {
    let p = PartialReasoningWindowConfig::default_values(&())
        .unwrap()
        .unwrap();

    assert_eq!(p.rows, Some(WindowRows::Off));
    assert_eq!(p.timer, Some(true));
}

#[test]
fn the_window_section_parses_from_toml() {
    let p: PartialAppConfig = toml::from_str(indoc! {"
        [style.reasoning.window]
        rows = 8
        timer = false
    "})
    .unwrap();

    assert_eq!(
        p.style.reasoning.window.rows,
        Some(WindowRows::Fixed(RowCount { rows: 8 }))
    );
    assert_eq!(p.style.reasoning.window.timer, Some(false));
}

#[test]
fn assign_window_fields_via_cli() {
    let mut p = PartialReasoningConfig::default();

    let kv = KvAssignment::try_from_cli("window.rows", "8").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.window.rows, Some(WindowRows::Fixed(RowCount { rows: 8 })));

    let kv = KvAssignment::try_from_cli("window.rows", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.window.rows, Some(WindowRows::Auto));

    let kv = KvAssignment::try_from_cli("window.rows", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.window.rows, Some(WindowRows::Off));

    let kv = KvAssignment::try_from_cli("window.timer", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.window.timer, Some(false));
}

/// `--cfg style.reasoning.window.rows:=4` hands over JSON rather than a string,
/// and the advertised spellings must not depend on which syntax typed them.
#[test]
fn assign_window_rows_as_json() {
    let mut p = PartialReasoningConfig::default();

    let kv = KvAssignment::try_from_cli("window.rows:", "4").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.window.rows, Some(WindowRows::Fixed(RowCount { rows: 4 })));

    let kv = KvAssignment::try_from_cli("window.rows:", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.window.rows, Some(WindowRows::Off));
}

/// A conversation records what changed between two configs, so a new window
/// size has to be recorded without the rest of the reasoning style.
#[test]
fn a_window_change_is_the_whole_delta() {
    let prev = AppConfig::new_test().style.reasoning.to_partial();
    let mut next = prev.clone();
    next.window.rows = Some(WindowRows::Fixed(RowCount { rows: 8 }));

    assert_eq!(prev.delta(next), PartialReasoningConfig {
        window: PartialReasoningWindowConfig {
            rows: Some(WindowRows::Fixed(RowCount { rows: 8 })),
            timer: None,
        },
        ..Default::default()
    });
}

#[test]
fn filling_keeps_a_set_window_field_and_fills_the_other() {
    let partial = PartialReasoningConfig {
        window: PartialReasoningWindowConfig {
            rows: Some(WindowRows::Auto),
            timer: None,
        },
        ..Default::default()
    };
    let defaults = PartialReasoningConfig::default_values(&())
        .unwrap()
        .unwrap();

    let filled = partial.fill_from(defaults);

    assert_eq!(filled.window.rows, Some(WindowRows::Auto));
    assert_eq!(filled.window.timer, Some(true));
}

#[test]
fn a_resolved_window_survives_the_trip_back_to_a_partial() {
    let mut config = AppConfig::new_test().style.reasoning;
    config.window.rows = WindowRows::Fixed(RowCount { rows: 8 });
    config.window.timer = false;

    let partial = config.to_partial();

    assert_eq!(
        partial.window.rows,
        Some(WindowRows::Fixed(RowCount { rows: 8 }))
    );
    assert_eq!(partial.window.timer, Some(false));
}
